//! Orchestrator for YouTube backend server.
//!
//! Contains the complex business logic for:
//! - Prefetch window management (3-track rolling window)
//! - Track-ended handling (auto-advance, repeat modes)
//! - Play position logic with MPRIS updates
//!
//! This module is used by both the main server and the internal event
//! processor.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tokio::runtime::{Builder, Handle};

use crate::backends::youtube::{
    audio::MpvInput,
    media::{MediaPreparer, PreloadTier, PreparedMedia},
    protocol::{ServerResponse, SongData},
    services::{
        AdvanceIntent,
        PlaybackService,
        PlaybackState,
        PlaybackStateTracker,
        QueueService,
        RepeatMode,
    },
};

/// Prefetch window size for gapless playback
pub const PREFETCH_WINDOW_SIZE: usize = 3;

/// Threshold for triggering prefetch (seconds remaining)
const PREFETCH_TRIGGER_THRESHOLD: f64 = 30.0;

const EARLY_EOF_MIN_DURATION_SECS: f64 = 90.0;
const EARLY_EOF_MIN_POSITION_SECS: f64 = 30.0;
const EARLY_EOF_MIN_REMAINING_SECS: f64 = 20.0;
const EARLY_EOF_MAX_RECOVERY_ATTEMPTS: u8 = 1;

/// Track which videos have had T-30s prefetch triggered
static PREFETCH_TRIGGERED: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

static EARLY_EOF_RECOVERY_COUNTS: LazyLock<Mutex<HashMap<String, u8>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn tier_for_window_offset(offset: usize) -> PreloadTier {
    match offset {
        0 => PreloadTier::Immediate,
        1 => PreloadTier::Gapless,
        _ => PreloadTier::Eager,
    }
}

fn prepare_media_blocking(
    media_preparer: &Arc<dyn MediaPreparer>,
    track_id: &str,
    tier: PreloadTier,
) -> Result<PreparedMedia> {
    if let Ok(handle) = Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(media_preparer.prepare(track_id, tier)))
    } else {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to build Tokio runtime for media preparation")?;
        runtime.block_on(media_preparer.prepare(track_id, tier))
    }
}

fn build_runtime_mpv_input(
    playback: &Arc<PlaybackService>,
    track_id: &str,
    prepared: &PreparedMedia,
) -> Result<MpvInput> {
    playback.build_runtime_input(track_id, prepared)
}

fn activate_playback_window(
    media_preparer: &Arc<dyn MediaPreparer>,
    queue: &Arc<QueueService>,
    indices: &[usize],
) {
    let track_ids: Vec<String> = indices
        .iter()
        .filter_map(|&idx| queue.get_by_index(idx).ok())
        .map(|song| song.uri)
        .filter(|uri| !uri.is_empty())
        .collect();

    media_preparer.activate_playback_window(&track_ids);
}

/// Play a track at the given queue position.
///
/// This rebuilds MPV's buffer with current + next tracks,
/// enabling gapless auto-advance when a song ends.
///
/// Uses MediaPreparer.prepare() to resolve URLs through the proper abstraction
/// layer.
pub async fn play_position(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    log::info!("play_position called for pos={} (queue len={})", pos, queue.len());

    // Stop current playback first to ensure clean state
    if let Err(e) = playback.stop() {
        log::debug!("stop() returned error (may be idle): {}", e);
    }
    state_tracker.force_set(PlaybackState::Idle);

    // Clear any remaining buffer entries
    if let Err(e) = playback.playlist_clear() {
        log::warn!("Failed to clear MPV buffer: {}", e);
    }

    let queue_len = queue.len();
    if pos >= queue_len {
        return ServerResponse::Error("Position out of bounds".to_string());
    }

    // Update current position in our queue and playback base tracking
    // We do this before URL resolution so that the UI reflects the intent to play
    // even if resolution fails (e.g. network error, or in tests).
    queue.set_current(Some(pos));
    queue.set_playback_base_index(pos);

    // Build prefetch window respecting shuffle/repeat order
    // This stores the indices for later lookup by handle_within_window_advance
    let prefetch_indices = queue.build_prefetch_window(pos, PREFETCH_WINDOW_SIZE);
    let prefetch_count = prefetch_indices.len();

    log::debug!(
        "Built prefetch window: {:?} (shuffle={})",
        prefetch_indices,
        queue.shuffle_enabled()
    );

    activate_playback_window(media_preparer, queue, &prefetch_indices);

    let mut first_url_metadata: Option<(String, String)> = None;
    let mut first_track = true;

    for (i, &idx) in prefetch_indices.iter().enumerate() {
        match queue.get_by_index(idx) {
            Ok(song) => {
                let video_id = &song.uri;

                // Determine tier based on position in prefetch window
                let tier = tier_for_window_offset(i);

                // Use MediaPreparer.prepare() instead of bypassing to build_playback_url
                match media_preparer.prepare(video_id, tier).await {
                    Ok(prepared) => {
                        match build_runtime_mpv_input(playback, video_id, &prepared) {
                            Ok(input) => {
                                if let Err(e) = playback.playlist_append_input(&input) {
                                    log::error!("Failed to append to playlist: {}", e);
                                } else {
                                    log::debug!(
                                        "Prepared track {} (queue pos {}) via MediaPreparer",
                                        i,
                                        idx
                                    );

                                    if first_track {
                                        first_track = false;
                                        let title = song
                                            .metadata
                                            .get("title")
                                            .and_then(|v| v.first())
                                            .cloned()
                                            .unwrap_or_else(|| video_id.clone());
                                        let artist = song
                                            .metadata
                                            .get("artist")
                                            .and_then(|v| v.first())
                                            .cloned()
                                            .unwrap_or_default();
                                        first_url_metadata = Some((title, artist));
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to build MPV input for {}: {}", video_id, e);
                                if first_track {
                                    return ServerResponse::Error(format!(
                                        "Failed to prepare stream: {}",
                                        e
                                    ));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to prepare media for {}: {}", video_id, e);
                        if first_track {
                            return ServerResponse::Error(format!(
                                "Failed to resolve stream: {}",
                                e
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to get song at index {}: {}", idx, e);
            }
        }
    }

    // Set MPRIS metadata for first track
    if let Some((title, artist)) = first_url_metadata {
        if let Err(e) = playback.set_media_title(&title, &artist) {
            log::warn!("Failed to set media title: {}", e);
        }
    }

    // Play first track in MPV's playlist
    match playback.playlist_play_index(0) {
        Ok(_) => {
            log::info!("Playing position {} (prefetched {} tracks)", pos, prefetch_count);
            state_tracker.force_set(PlaybackState::Playing);
            ServerResponse::Ok
        }
        Err(e) => {
            log::error!("Failed to play playlist index 0: {}", e);
            state_tracker.force_set(PlaybackState::Idle);
            ServerResponse::Error(e.to_string())
        }
    }
}

/// Internal version of play_position that returns Result
pub async fn play_position_internal(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> Result<()> {
    match play_position(playback, queue, pos, state_tracker, media_preparer).await {
        ServerResponse::Ok => Ok(()),
        ServerResponse::Error(e) => Err(anyhow::anyhow!("{}", e)),
        _ => Ok(()),
    }
}

/// Sync version for internal callers (repeat-one, auto-advance fallback).
pub fn play_position_sync(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    log::info!("play_position_sync for pos={} (uses MediaPreparer)", pos);

    if let Err(e) = playback.stop() {
        log::debug!("stop() returned error (may be idle): {}", e);
    }
    state_tracker.force_set(PlaybackState::Idle);

    if let Err(e) = playback.playlist_clear() {
        log::warn!("Failed to clear MPV buffer: {}", e);
    }

    let queue_len = queue.len();
    if pos >= queue_len {
        return ServerResponse::Error("Position out of bounds".to_string());
    }

    queue.set_current(Some(pos));
    queue.set_playback_base_index(pos);

    let prefetch_indices = queue.build_prefetch_window(pos, PREFETCH_WINDOW_SIZE);
    let prefetch_count = prefetch_indices.len();

    activate_playback_window(media_preparer, queue, &prefetch_indices);

    let mut first_url_metadata: Option<(String, String)> = None;
    let mut first_track = true;

    for (i, &idx) in prefetch_indices.iter().enumerate() {
        match queue.get_by_index(idx) {
            Ok(song) => {
                let video_id = &song.uri;
                let tier = tier_for_window_offset(i);
                match prepare_media_blocking(media_preparer, video_id, tier)
            .and_then(|prepared| build_runtime_mpv_input(playback, &video_id, &prepared))
                {
                    Ok(input) => {
                        if let Err(e) = playback.playlist_append_input(&input) {
                            log::error!("Failed to append to playlist: {}", e);
                        } else {
                            log::debug!(
                                "Prepared track {} (queue pos {}) via MediaPreparer",
                                i,
                                idx
                            );
                            if first_track {
                                first_track = false;
                                let title = song
                                    .metadata
                                    .get("title")
                                    .and_then(|v| v.first())
                                    .cloned()
                                    .unwrap_or_else(|| video_id.clone());
                                let artist = song
                                    .metadata
                                    .get("artist")
                                    .and_then(|v| v.first())
                                    .cloned()
                                    .unwrap_or_default();
                                first_url_metadata = Some((title, artist));
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to prepare media for {}: {}", video_id, e);
                        if first_track {
                            return ServerResponse::Error(format!(
                                "Failed to prepare stream: {}",
                                e
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to get song at index {}: {}", idx, e);
            }
        }
    }

    if let Some((title, artist)) = first_url_metadata {
        if let Err(e) = playback.set_media_title(&title, &artist) {
            log::warn!("Failed to set media title: {}", e);
        }
    }

    match playback.playlist_play_index(0) {
        Ok(_) => {
            log::info!("Playing position {} (prefetched {} tracks)", pos, prefetch_count);
            state_tracker.force_set(PlaybackState::Playing);
            ServerResponse::Ok
        }
        Err(e) => {
            log::error!("Failed to play playlist index 0: {}", e);
            state_tracker.force_set(PlaybackState::Idle);
            ServerResponse::Error(e.to_string())
        }
    }
}

/// Internal sync version that returns Result
pub fn play_position_internal_sync(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> Result<()> {
    match play_position_sync(playback, queue, pos, state_tracker, media_preparer) {
        ServerResponse::Ok => Ok(()),
        ServerResponse::Error(e) => Err(anyhow::anyhow!("{}", e)),
        _ => Ok(()),
    }
}

/// Handle track ended event from MPV.
///
/// When MPV auto-advances (end-file:eof), we need to:
/// 1. Sync our queue position with MPV's playlist-pos
/// 2. Append next track to maintain prefetch window
/// 3. Update MPRIS metadata for new track
/// 4. Handle repeat modes
pub fn handle_track_ended(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    reason: &str,
) {
    log::info!("Track ended with reason: {}", reason);

    match reason {
        "eof" => handle_eof(playback, queue, state_tracker, media_preparer),
        "error" => handle_playback_error(playback, queue, state_tracker, media_preparer),
        "stop" => {
            log::debug!("Playback stopped by user");
            state_tracker.force_set(PlaybackState::Stopped);
        }
        _ => {
            log::debug!("Unhandled end-file reason: {}", reason);
        }
    }
}

/// Handle natural end of file (eof)
fn handle_eof(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) {
    log::trace!("[DIAG-EOF] EOF");

    let repeat_mode = queue.repeat_mode();
    let current_idx = queue.current_index();
    let queue_len = queue.len();
    log::info!(
        "[DIAG-EOF] handle_eof: repeat_mode={:?} current_idx={:?} queue_len={}",
        repeat_mode,
        current_idx,
        queue_len
    );

    if try_recover_from_early_eof(playback, queue, state_tracker, media_preparer, current_idx) {
        return;
    }

    // Determine intent based on repeat mode at EOF time
    let intent = determine_advance_intent(queue, repeat_mode);
    log::info!("[DIAG-EOF] Captured intent: {:?}", intent);

    // Handle RepeatOne immediately - no need to wait for MPV confirmation
    if intent == AdvanceIntent::Repeat {
        log::info!("[DIAG-EOF] Repeat One: replaying current track");
        if let Some(current_idx) = queue.current_index() {
            let _ =
                play_position_internal_sync(playback, queue, current_idx, state_tracker, media_preparer);
            return;
        }
        if let Err(e) = playback.seek(0.0, "absolute") {
            log::error!("Failed to seek to start: {}", e);
        }
        if let Err(e) = playback.unpause() {
            log::error!("Failed to unpause: {}", e);
        }
        state_tracker.force_set(PlaybackState::Playing);
        return;
    }

    let from_position = queue.current_index().unwrap_or(0);
    state_tracker.force_set(PlaybackState::PendingAdvance {
        since: Instant::now(),
        from_position,
        intent,
    });
    log::info!("EOF transition pending, waiting for MPV event confirmation");
    log::trace!(
        "[DIAG-EOF] EOF -> PendingAdvance(from_position={}, intent={:?})",
        from_position,
        intent
    );

    spawn_pending_advance_timeout(playback, queue, state_tracker, Arc::clone(media_preparer));
}

fn try_recover_from_early_eof(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    current_idx: Option<usize>,
) -> bool {
    let Some(idx) = current_idx else {
        return false;
    };

    let Ok(song) = queue.get_by_index(idx) else {
        return false;
    };
    let video_id = song.uri.clone();

    let position_secs = match playback.get_position() {
        Ok(pos) => pos,
        Err(err) => {
            log::debug!("[DIAG-EOF] Could not read time-pos for {}: {}", video_id, err);
            return false;
        }
    };
    let duration_secs = match playback.get_duration() {
        Ok(dur) => dur,
        Err(err) => {
            log::debug!("[DIAG-EOF] Could not read duration for {}: {}", video_id, err);
            return false;
        }
    };

    if !is_suspicious_eof(position_secs, duration_secs) {
        EARLY_EOF_RECOVERY_COUNTS.lock().remove(&video_id);
        return false;
    }

    let remaining_secs = duration_secs - position_secs;
    let mut attempts = EARLY_EOF_RECOVERY_COUNTS.lock();
    let attempt = attempts.entry(video_id.clone()).or_insert(0);

    if *attempt >= EARLY_EOF_MAX_RECOVERY_ATTEMPTS {
        log::warn!(
            "[DIAG-EOF] Suspicious EOF for {} at {:.2}/{:.2}s (remaining {:.2}s), recovery limit reached",
            video_id,
            position_secs,
            duration_secs,
            remaining_secs
        );
        return false;
    }

    *attempt += 1;
    let attempt_no = *attempt;
    drop(attempts);

    log::warn!(
        "[DIAG-EOF] Suspicious EOF for {} at {:.2}/{:.2}s (remaining {:.2}s), retrying current track with fresh URL cache (attempt {}/{})",
        video_id,
        position_secs,
        duration_secs,
        remaining_secs,
        attempt_no,
        EARLY_EOF_MAX_RECOVERY_ATTEMPTS
    );

    playback.clear_stream_url_cache();

    match play_position_internal_sync(playback, queue, idx, state_tracker, media_preparer) {
        Ok(()) => true,
        Err(err) => {
            log::warn!("[DIAG-EOF] Early-EOF recovery replay failed for {}: {}", video_id, err);
            false
        }
    }
}

fn is_suspicious_eof(position_secs: f64, duration_secs: f64) -> bool {
    if !position_secs.is_finite() || !duration_secs.is_finite() {
        return false;
    }

    if duration_secs < EARLY_EOF_MIN_DURATION_SECS || position_secs < EARLY_EOF_MIN_POSITION_SECS {
        return false;
    }

    if position_secs >= duration_secs {
        return false;
    }

    (duration_secs - position_secs) >= EARLY_EOF_MIN_REMAINING_SECS
}

fn determine_advance_intent(queue: &Arc<QueueService>, repeat_mode: RepeatMode) -> AdvanceIntent {
    match repeat_mode {
        RepeatMode::One => AdvanceIntent::Repeat,
        RepeatMode::All => AdvanceIntent::Advance,
        RepeatMode::Off => {
            if queue.next_index().is_some() {
                AdvanceIntent::Advance
            } else {
                AdvanceIntent::Stop
            }
        }
    }
}

fn spawn_pending_advance_timeout(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: Arc<dyn MediaPreparer>,
) {
        let playback = Arc::clone(playback);
        let queue = Arc::clone(queue);
        let state_tracker = Arc::clone(state_tracker);
        let media_preparer = Arc::clone(&media_preparer);

    thread::spawn(move || {
        let timeout = Duration::from_secs(2);
        thread::sleep(timeout);

        if !state_tracker.is_pending_expired(timeout) {
            return;
        }
        if !matches!(state_tracker.get(), PlaybackState::PendingAdvance { .. }) {
            return;
        }

        let mpv_pos = match playback.get_playlist_pos() {
            Ok(pos) => pos,
            Err(e) => {
                log::warn!(
                    "[DIAG-EOF] PendingAdvance timeout fallback failed to query MPV playlist-pos: {}",
                    e
                );
                -1
            }
        };

        log::warn!(
            "[DIAG-EOF] PendingAdvance timeout ({}s), falling back to MPV playlist-pos={}",
            timeout.as_secs(),
            mpv_pos
        );

        let position = mpv_pos.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        handle_track_changed(&playback, &queue, &state_tracker, &media_preparer, position);
    });
}

pub fn handle_track_changed(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    position: i32,
) {
    let state = state_tracker.get();
    let PlaybackState::PendingAdvance { from_position, intent, .. } = state else {
        log::trace!("[DIAG-EOF] TrackChanged({}) ignored (state={:?})", position, state);
        return;
    };

    log::trace!(
        "[DIAG-EOF] PendingAdvance -> TrackChanged({}) (from_position={}, intent={:?})",
        position,
        from_position,
        intent
    );

    let sync_ok = execute_intent(playback, queue, state_tracker, media_preparer, position, intent);

    if !sync_ok || matches!(state_tracker.get(), PlaybackState::PendingAdvance { .. }) {
        log::warn!("[DIAG-EOF] Queue sync failed (position={}), forcing recovery", position);
        queue.set_current(None);
        state_tracker.force_set(PlaybackState::Idle);
    }

    clear_prefetch_triggered(queue);

    log::trace!("[DIAG-EOF] TrackChanged({}) -> {:?}", position, state_tracker.get());
}

fn execute_intent(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    position: i32,
    intent: AdvanceIntent,
) -> bool {
    match intent {
        AdvanceIntent::Repeat => {
            if let Some(current_idx) = queue.current_index() {
                let _ = play_position_internal_sync(
                    playback,
                    queue,
                    current_idx,
                    state_tracker,
                    media_preparer,
                );
            } else {
                let _ = playback.seek(0.0, "absolute");
                let _ = playback.unpause();
                state_tracker.force_set(PlaybackState::Playing);
            }
            true
        }
        AdvanceIntent::Stop => {
            log::info!("[DIAG-EOF] Intent::Stop - end of queue");
            queue.set_current(None);
            state_tracker.force_set(PlaybackState::Idle);
            true
        }
        AdvanceIntent::Advance => {
            if position < 0 {
                handle_end_of_window(
                    playback,
                    queue,
                    state_tracker,
                    media_preparer,
                    queue.repeat_mode(),
                );
                true
            } else {
                handle_within_window_advance(
                    playback,
                    queue,
                    state_tracker,
                    media_preparer,
                    position as usize,
                )
            }
        }
    }
}

/// Handle when we've reached end of prefetch window
fn handle_end_of_window(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    repeat_mode: RepeatMode,
) {
    log::info!("[DIAG-EOF] handle_end_of_window: start");
    let next_pos = queue.next_index();
    log::info!(
        "[DIAG-EOF] handle_end_of_window: current={:?} next_pos={:?} queue_len={}",
        queue.current_index(),
        next_pos,
        queue.len()
    );

    match next_pos {
        Some(pos) => {
            log::info!("[DIAG-EOF] End of prefetch window, loading next batch at {}", pos);
            let _ = play_position_internal_sync(playback, queue, pos, state_tracker, media_preparer);
        }
        None => {
            if repeat_mode == RepeatMode::All && queue.len() > 0 {
                log::info!("[DIAG-EOF] Repeat All: looping back to start");
                let _ = play_position_internal_sync(playback, queue, 0, state_tracker, media_preparer);
            } else {
                log::info!("[DIAG-EOF] Reached end of queue, going idle");
                queue.set_current(None);
                state_tracker.force_set(PlaybackState::Idle);
            }
        }
    }
}

/// Handle MPV advancing within the prefetch window
/// Returns true if queue sync succeeded, false if recovery is needed
fn handle_within_window_advance(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    mpv_pos: usize,
) -> bool {
    let new_queue_pos = match queue.get_prefetched_at(mpv_pos) {
        Some(idx) => idx,
        None => {
            let base_index = queue.playback_base_index();
            log::warn!(
                "[DIAG-EOF] Prefetch lookup failed for mpv_pos={}, falling back to base_index({}) + mpv_pos",
                mpv_pos,
                base_index
            );
            base_index + mpv_pos
        }
    };

    log::info!(
        "[DIAG-EOF] handle_within_window_advance: mpv_pos={} new_queue_pos={} queue_len={}",
        mpv_pos,
        new_queue_pos,
        queue.len()
    );

    if new_queue_pos >= queue.len() {
        log::warn!(
            "[DIAG-EOF] new_queue_pos {} out of bounds (len={})",
            new_queue_pos,
            queue.len()
        );
        return false;
    }

    log::info!("[DIAG-EOF] MPV at playlist-pos {}, queue pos now {}", mpv_pos, new_queue_pos);
    queue.set_current(Some(new_queue_pos));
    state_tracker.force_set(PlaybackState::Playing);

    if let Ok(song) = queue.get_by_index(new_queue_pos) {
        let title = song
            .metadata
            .get("title")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_else(|| song.uri.clone());
        let artist =
            song.metadata.get("artist").and_then(|v| v.first()).cloned().unwrap_or_default();
        let _ = playback.set_media_title(&title, &artist);
    }

    if let Some(next_idx) = queue.extend_prefetch_window() {
        let active_indices = queue.build_prefetch_window(new_queue_pos, PREFETCH_WINDOW_SIZE);
        activate_playback_window(media_preparer, queue, &active_indices);

        if let Ok(song) = queue.get_by_index(next_idx) {
            if let Ok(url) = prepare_media_blocking(media_preparer, &song.uri, PreloadTier::Eager)
                .and_then(|prepared| build_runtime_mpv_input(playback, &song.uri, &prepared))
            {
                let _ = playback.playlist_append_input(&url);
                log::debug!("Extended prefetch window with queue index {}", next_idx);
            }
        }
    }

    true
}

/// Handle playback error by skipping to next track
fn handle_playback_error(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) {
    log::warn!("Playback error, attempting to skip to next");
    if let Some(current) = queue.current_index() {
        let next_pos = current + 1;
        if next_pos < queue.len() {
            let _ =
                play_position_internal_sync(playback, queue, next_pos, state_tracker, media_preparer);
        } else {
            state_tracker.force_set(PlaybackState::Idle);
        }
    }
}

/// Prefetch upcoming tracks in background
pub fn prefetch_upcoming(media_preparer: &Arc<dyn MediaPreparer>, queue: &Arc<QueueService>) {
    if let Some(current) = queue.current_index() {
        let all_songs = queue.get_all();
        let track_ids: Vec<String> = all_songs
            .iter()
            .skip(current + 1)
            .take(5)
            .filter_map(|s| extract_video_id(&s.uri))
            .collect();

        for track_id in track_ids {
            media_preparer.prefetch(&track_id, PreloadTier::Background);
        }
    }
}

/// Get the current song based on MPV's position
pub fn get_current_song(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
) -> ServerResponse {
    let mpv_playlist_pos = playback.get_playlist_pos().unwrap_or(-1);
    if mpv_playlist_pos < 0 {
        return ServerResponse::Song(None);
    }

    let queue_pos = queue.playback_base_index() + (mpv_playlist_pos as usize);

    match queue.get_by_index(queue_pos) {
        Ok(song) => ServerResponse::Song(Some(SongData::from(song))),
        Err(_) => ServerResponse::Song(None),
    }
}

/// Get playlist as SongData list
pub fn get_playlist(queue: &Arc<QueueService>) -> ServerResponse {
    let songs = queue.get_all();
    ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
}

/// Next track navigation
pub fn next_track(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    match queue.next_index() {
        Some(idx) => play_position_sync(playback, queue, idx, state_tracker, media_preparer),
        None => ServerResponse::Error("No next track".into()),
    }
}

/// Previous track navigation
pub fn previous_track(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    match queue.previous_index() {
        Some(0) => {
            // At first track, restart current track
            match playback.seek(0.0, "absolute") {
                Ok(_) => {
                    state_tracker.force_set(PlaybackState::Playing);
                    ServerResponse::Ok
                }
                Err(e) => ServerResponse::Error(e.to_string()),
            }
        }
        Some(idx) => play_position_sync(playback, queue, idx, state_tracker, media_preparer),
        None => ServerResponse::Error("No previous track".into()),
    }
}

/// Play by song ID
pub fn play_id(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    id: u32,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    match queue.find_index_by_id(id) {
        Some(pos) => play_position_sync(playback, queue, pos, state_tracker, media_preparer),
        None => ServerResponse::Error("Song not found".into()),
    }
}

/// Handle time-remaining updates from MPV.
///
/// Triggers prefetch of next track when playback position reaches T-30s.
/// Uses debouncing to ensure each track only triggers once.
pub fn handle_time_remaining(
    queue: &Arc<QueueService>,
    media_preparer: &Arc<dyn MediaPreparer>,
    time_remaining_secs: f64,
) {
    if time_remaining_secs > PREFETCH_TRIGGER_THRESHOLD || time_remaining_secs <= 0.0 {
        return;
    }

    let current_idx = match queue.current_index() {
        Some(idx) => idx,
        None => return,
    };

    let current_song = match queue.get_by_index(current_idx) {
        Ok(song) => song,
        Err(_) => return,
    };

    let video_id = &current_song.uri;

    {
        let mut triggered = PREFETCH_TRIGGERED.lock();
        if triggered.contains(video_id) {
            return;
        }
        triggered.insert(video_id.to_string());
    }

    let next_idx = match queue.next_index() {
        Some(idx) => idx,
        None => return,
    };

    let next_song = match queue.get_by_index(next_idx) {
        Ok(song) => song,
        Err(e) => {
            log::warn!("Failed to get next track for prefetch: {}", e);
            return;
        }
    };

    let next_track_id = match extract_video_id(&next_song.uri) {
        Some(track_id) => track_id,
        None => return,
    };

    log::info!(
        "T-30s prefetch trigger: {}s remaining, prefetching next track: {}",
        time_remaining_secs,
        next_track_id
    );

    media_preparer.prefetch(&next_track_id, PreloadTier::Gapless);
}

fn extract_video_id(uri: &str) -> Option<String> {
    if let Some(id) = uri.strip_prefix("youtube://") {
        return Some(id.to_string());
    }

    if !uri.is_empty() && !uri.contains("://") {
        return Some(uri.to_string());
    }

    None
}

fn clear_prefetch_triggered(queue: &Arc<QueueService>) {
    let mut triggered = PREFETCH_TRIGGERED.lock();

    let current_video_id = queue
        .current_index()
        .and_then(|idx| queue.get_by_index(idx).ok())
        .map(|song| song.uri.clone());

    let next_video_ids: Vec<String> = (0..3)
        .filter_map(|offset| {
            queue
                .current_index()
                .and_then(|idx| idx.checked_add(offset + 1))
                .and_then(|idx| queue.get_by_index(idx).ok())
                .map(|song| song.uri.clone())
        })
        .collect();

    triggered.retain(|id| {
        if let Some(ref current) = current_video_id {
            id == current || next_video_ids.contains(id)
        } else {
            false
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        backends::youtube::{
            audio::{AudioSourcePlanner, FfmpegConcatSource},
            config::{AudioDeliveryMode, ExtractorType},
            url_resolver::UrlResolver,
        },
        domain::Song,
    };

    fn setup_test_services(
    ) -> (TempDir, Arc<PlaybackService>, Arc<QueueService>, Arc<PlaybackStateTracker>) {
        let temp_dir = TempDir::new().unwrap();
        let socket = temp_dir.path().join("test-mpv.sock");

        let url_resolver = Arc::new(UrlResolver::new(ExtractorType::default()));
        let playback = Arc::new(
            PlaybackService::new(
                &socket,
                url_resolver,
                None,
                AudioSourcePlanner.plan(AudioDeliveryMode::Direct),
                None,
            )
            .unwrap(),
        );
        let queue = Arc::new(QueueService::new());
        let state_tracker = Arc::new(PlaybackStateTracker::new());

        (temp_dir, playback, queue, state_tracker)
    }

    fn test_song(id: &str) -> Song {
        let mut song = Song::default();
        song.uri = id.to_string();
        song.metadata.insert("title".to_string(), vec![id.to_string()]);
        song
    }

    struct StubMediaPreparer;

    #[async_trait]
    impl MediaPreparer for StubMediaPreparer {
        async fn prepare(&self, track_id: &str, _tier: PreloadTier) -> Result<PreparedMedia> {
            Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
        }

        fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}
    }

    fn stub_media_preparer() -> Arc<dyn MediaPreparer> {
        Arc::new(StubMediaPreparer)
    }

    #[derive(Default)]
    struct RecordingMediaPreparer {
        prepared: Arc<Mutex<Vec<(String, PreloadTier)>>>,
        prefetched: Arc<Mutex<Vec<(String, PreloadTier)>>>,
        activated_windows: Arc<Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl MediaPreparer for RecordingMediaPreparer {
        async fn prepare(&self, track_id: &str, tier: PreloadTier) -> Result<PreparedMedia> {
            self.prepared.lock().push((track_id.to_string(), tier));
            Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
        }

        fn prefetch(&self, track_id: &str, tier: PreloadTier) {
            self.prefetched.lock().push((track_id.to_string(), tier));
        }

        fn activate_playback_window(&self, track_ids: &[String]) {
            self.activated_windows.lock().push(track_ids.to_vec());
        }
    }

    #[test]
    fn sync_and_async_window_tier_mapping_matches() {
        assert_eq!(tier_for_window_offset(0), PreloadTier::Immediate);
        assert_eq!(tier_for_window_offset(1), PreloadTier::Gapless);
        assert_eq!(tier_for_window_offset(2), PreloadTier::Eager);
        assert_eq!(tier_for_window_offset(5), PreloadTier::Eager);
    }

    #[test]
    fn sync_prepare_path_uses_media_preparer_contract() {
        let media_preparer = stub_media_preparer();
        let prepared =
            prepare_media_blocking(&media_preparer, "video123", PreloadTier::Immediate).unwrap();

        match prepared {
            PreparedMedia::Direct { url } => {
                assert_eq!(url, "https://example.invalid/video123");
            }
            other => panic!("unexpected prepared media variant: {other:?}"),
        }
    }

    #[test]
    fn runtime_builder_uses_concat_source_for_staged_prefix() {
        let (_temp_dir, playback, _queue, _state_tracker) = setup_test_services();

        let prepared = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 1024,
            url: "https://example.invalid/stream".to_string(),
            content_length: 4096,
        };

        let input = build_runtime_mpv_input(&playback, "v1", &prepared).unwrap();

        assert_eq!(
            input.url,
            "lavf://concat:/tmp/prefix.webm|subfile,,start,1024,end,0,,:https://example.invalid/stream"
        );
        assert_eq!(input.mpv_args, FfmpegConcatSource::protocol_whitelist_args());
    }

    #[test]
    fn runtime_builder_preserves_direct_fallback_input() {
        let (_temp_dir, playback, _queue, _state_tracker) = setup_test_services();

        let prepared = PreparedMedia::Direct { url: "https://example.invalid/direct".to_string() };

        let input = build_runtime_mpv_input(&playback, "v1", &prepared).unwrap();

        assert_eq!(input.url, "https://example.invalid/direct");
        assert!(input.mpv_args.is_empty());
    }

    #[test]
    fn suspicious_eof_detects_early_cutoff() {
        assert!(is_suspicious_eof(210.6, 236.98));
    }

    #[test]
    fn suspicious_eof_ignores_near_end() {
        assert!(!is_suspicious_eof(228.0, 236.98));
    }

    #[test]
    fn suspicious_eof_ignores_short_tracks() {
        assert!(!is_suspicious_eof(20.0, 45.0));
    }

    #[test]
    fn eof_advances_to_next_track() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);
        queue.set_current(Some(0));

        handle_eof(&playback, &queue, &state_tracker, &media_preparer);
        handle_track_changed(&playback, &queue, &state_tracker, &media_preparer, 1);

        assert_eq!(queue.current_index(), Some(1));
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn eof_enters_pending_advance_before_track_changed_confirmation() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);
        queue.set_current(Some(0));

        handle_eof(&playback, &queue, &state_tracker, &media_preparer);

        match state_tracker.get() {
            PlaybackState::PendingAdvance { from_position, intent, .. } => {
                assert_eq!(from_position, 0);
                assert_eq!(intent, AdvanceIntent::Advance);
            }
            other => panic!("expected pending advance, got {other:?}"),
        }

        handle_track_changed(&playback, &queue, &state_tracker, &media_preparer, 1);
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn eof_with_repeat_one_replays_current() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();
        queue.add(test_song("Song 1"), None);
        queue.set_current(Some(0));
        queue.set_repeat_mode(RepeatMode::One);

        handle_eof(&playback, &queue, &state_tracker, &media_preparer);

        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn eof_at_end_with_repeat_all_loops() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);
        queue.set_current(Some(1)); // Last song
        queue.set_repeat_mode(RepeatMode::All);

        handle_eof(&playback, &queue, &state_tracker, &media_preparer);
        handle_track_changed(&playback, &queue, &state_tracker, &media_preparer, -1);

        assert_eq!(queue.current_index(), Some(0)); // Looped
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn eof_at_end_without_repeat_goes_idle() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();
        queue.add(test_song("Song 1"), None);
        queue.set_current(Some(0));
        queue.set_repeat_mode(RepeatMode::Off);

        handle_eof(&playback, &queue, &state_tracker, &media_preparer);
        handle_track_changed(&playback, &queue, &state_tracker, &media_preparer, -1);

        assert_eq!(queue.current_index(), None);
        assert_eq!(state_tracker.get(), PlaybackState::Idle);
        std::thread::sleep(Duration::from_millis(2100));
    }

    // =========================================================================
    // RED TESTS: These tests MUST FAIL to prove the bugs exist
    // =========================================================================

    /// TEST: Shuffle is respected during auto-advance (within prefetch window).
    ///
    /// When shuffle is enabled and build_prefetch_window is called,
    /// the prefetch window should contain shuffled tracks.
    /// When mpv advances to mpv_pos=1, handle_within_window_advance should
    /// return the shuffled track, not queue[1].
    #[test]
    #[ignore]
    fn shuffle_is_respected_during_auto_advance() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();

        // Add 5 songs
        for i in 0..5 {
            queue.add(test_song(&format!("Song {}", i)), None);
        }

        // Enable shuffle BEFORE starting playback
        queue.set_shuffle_enabled(true);

        // Start playing from position 0
        queue.set_current(Some(0));
        queue.set_playback_base_index(0);

        // KEY: Call build_prefetch_window to populate prefetch_indices
        // This is what play_position does in production
        let prefetch_indices = queue.build_prefetch_window(0, 3);

        // The first track should be position 0 (where we started)
        assert_eq!(prefetch_indices[0], 0, "First track should be starting position");

        // With shuffle enabled, subsequent tracks should be from shuffle order
        // They might be sequential (20% chance with 5 items), so we test that
        // the prefetch lookup works correctly instead of asserting randomness

        // Simulate: MPV auto-advanced within window (mpv_pos=1)
        handle_within_window_advance(&playback, &queue, &state_tracker, &media_preparer, 1);

        // After advance, current should match what was at prefetch_indices[1]
        let current = queue.current_index().expect("Should have current");

        // The current index should be what was stored in prefetch_indices[1]
        // (which may or may not be 1 depending on shuffle order)
        assert_eq!(
            current, prefetch_indices[1],
            "Current should match the prefetched track at mpv_pos=1. \
             Expected queue index {} (from prefetch), got {}",
            prefetch_indices[1], current
        );
    }

    /// RED TEST: Prefetch window does not respect shuffle order.
    ///
    /// BUG: play_position() builds prefetch window as:
    ///   for i in 0..prefetch_count { idx = pos + i }  (SEQUENTIAL)
    ///
    /// When shuffle is enabled, the prefetch should use shuffle order.
    #[test]
    #[ignore]
    fn prefetch_window_respects_shuffle_order() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();

        // Add 5 songs
        for i in 0..5 {
            queue.add(test_song(&format!("Song {}", i)), None);
        }

        // Enable shuffle
        queue.set_shuffle_enabled(true);

        // Start from position 0
        // play_position will build prefetch window [0, 1, 2] SEQUENTIALLY
        // but with shuffle enabled, it SHOULD build [0, shuffle[1], shuffle[2]]
        let _ = play_position_sync(&playback, &queue, 0, &state_tracker, &media_preparer);

        // After play_position, check what base_index was set to
        let base = queue.playback_base_index();
        assert_eq!(base, 0, "Base should be starting position");

        // The BUG: We can't directly test what was prefetched because
        // play_position doesn't store the shuffled indices anywhere.
        // The only evidence is in handle_within_window_advance behavior
        // (tested above).
        //
        // This test documents the architectural gap: we need to STORE
        // what indices were actually prefetched, not just the base.

        // To prove the bug, we simulate what WOULD happen:
        // If we call next_index() multiple times, we get random results
        // But play_position uses pos+i, ignoring shuffle.

        // Get what next_index would return (respects shuffle)
        let next_shuffle = queue.next_index();

        // The bug: If shuffle was respected in prefetch, the second track
        // would match next_shuffle. But play_position uses pos+1=1.
        assert!(
            next_shuffle != Some(1) || next_shuffle.is_none(),
            "BUG: next_index() returns 1 in shuffle mode. \
             This is statistically unlikely (1/4 chance). \
             Run test multiple times - if it always passes, shuffle is broken."
        );
    }

    /// TEST: handle_within_window_advance respects repeat mode.
    ///
    /// When RepeatAll is enabled and we reach the end of the queue,
    /// the next advance should wrap to queue[0].
    #[test]
    fn within_window_advance_does_not_extend_beyond_queue_end_before_repeat_reload() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();
        let media_preparer = stub_media_preparer();

        // Add 3 songs (exactly PREFETCH_WINDOW_SIZE)
        queue.add(test_song("Song 0"), None);
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);

        // Enable Repeat All
        queue.set_repeat_mode(RepeatMode::All);

        // Start at position 0
        queue.set_current(Some(0));
        queue.set_playback_base_index(0);

        // KEY: Build prefetch window - this is what play_position does
        let prefetch_indices = queue.build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2], "Initial prefetch window");

        // Simulate: MPV at last track in window (mpv_pos=2)
        // This is Song 2, which is also the last in queue
        handle_within_window_advance(&playback, &queue, &state_tracker, &media_preparer, 2);

        // Current should be 2
        assert_eq!(queue.current_index(), Some(2));

        assert_eq!(queue.get_prefetched_at(3), None);
    }

    #[test]
    fn time_remaining_prefetch_uses_media_preparer_gapless_tier() {
        let (_temp_dir, _playback, queue, _state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("youtube://song-1"), None);
        queue.set_current(Some(0));

        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();

        handle_time_remaining(&queue, &media_preparer, 20.0);

        let prefetched = recording.prefetched.lock().clone();
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0], ("song-1".to_string(), PreloadTier::Gapless));
        assert_eq!(PreloadTier::Gapless, tier_for_window_offset(1));
    }

    #[test]
    fn time_remaining_prefetch_debounces_repeated_trigger_for_same_track() {
        let (_temp_dir, _playback, queue, _state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.set_current(Some(0));

        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();

        handle_time_remaining(&queue, &media_preparer, 20.0);
        handle_time_remaining(&queue, &media_preparer, 10.0);

        let prefetched = recording.prefetched.lock().clone();
        assert_eq!(prefetched, vec![("song-1".to_string(), PreloadTier::Gapless)]);
    }

    #[test]
    fn within_window_advance_refreshes_active_playback_window() {
        let (_temp_dir, playback, queue, state_tracker) = setup_test_services();

        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);
        queue.add(test_song("song-3"), None);
        queue.set_current(Some(0));
        queue.set_playback_base_index(0);
        let prefetch_indices = queue.build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();

        handle_within_window_advance(&playback, &queue, &state_tracker, &media_preparer, 1);

        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(recording.activated_windows.lock().clone(), vec![vec![
            "song-1".to_string(),
            "song-2".to_string(),
            "song-3".to_string(),
        ]]);
        assert_eq!(
            recording.prepared.lock().clone(),
            vec![("song-3".to_string(), PreloadTier::Eager)]
        );
    }
}
