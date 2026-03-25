use std::{
    collections::HashMap,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use crate::backends::youtube::{
    protocol::ServerResponse,
    server::{
        handlers::stable_track_id,
        orchestrator::PREFETCH_WINDOW_SIZE,
        playback_prepare::{build_runtime_mpv_input, prepare_media_blocking},
        prefetch_manager::PrefetchManager,
    },
    services::{
        AdvanceIntent,
        PlaybackService,
        PlaybackState,
        PlaybackStateTracker,
        QueueService,
        RepeatMode,
    },
};

const EARLY_EOF_MIN_DURATION_SECS: f64 = 90.0;
const EARLY_EOF_MIN_POSITION_SECS: f64 = 30.0;
const EARLY_EOF_MIN_REMAINING_SECS: f64 = 20.0;
const EARLY_EOF_MAX_RECOVERY_ATTEMPTS: u8 = 1;

pub(crate) fn is_suspicious_eof(position_secs: f64, duration_secs: f64) -> bool {
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

pub struct TrackEndedHandler {
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    state_tracker: Arc<PlaybackStateTracker>,
    media_preparer: Arc<dyn crate::backends::youtube::media::MediaPreparer>,
    prefetch_manager: Arc<PrefetchManager>,
    play_position_sync: Arc<dyn Fn(usize) -> ServerResponse + Send + Sync>,
    early_eof_counts: Arc<Mutex<HashMap<String, u8>>>,
}

impl TrackEndedHandler {
    pub fn new(
        playback: Arc<PlaybackService>,
        queue: Arc<QueueService>,
        state_tracker: Arc<PlaybackStateTracker>,
        media_preparer: Arc<dyn crate::backends::youtube::media::MediaPreparer>,
        prefetch_manager: Arc<PrefetchManager>,
        play_position_sync: Arc<dyn Fn(usize) -> ServerResponse + Send + Sync>,
    ) -> Self {
        Self {
            playback,
            queue,
            state_tracker,
            media_preparer,
            prefetch_manager,
            play_position_sync,
            early_eof_counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn handle_track_ended(&self, reason: &str) {
        log::info!("Track ended with reason: {}", reason);
        match reason {
            "eof" => self.handle_eof(),
            "error" => self.handle_playback_error(),
            "stop" => {
                log::debug!("Playback stopped by user");
                self.state_tracker.force_set(PlaybackState::Stopped);
            }
            _ => {
                log::debug!("Unhandled end-file reason: {}", reason);
            }
        }
    }

    pub fn handle_track_changed(&self, position: i32) {
        let state = self.state_tracker.get();
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

        let sync_ok = self.execute_intent(position, intent);

        if !sync_ok || matches!(self.state_tracker.get(), PlaybackState::PendingAdvance { .. }) {
            log::warn!("[DIAG-EOF] Queue sync failed (position={}), forcing recovery", position);
            self.queue.set_current(None);
            self.state_tracker.force_set(PlaybackState::Idle);
        }

        self.prefetch_manager.clear_triggered();
        log::trace!("[DIAG-EOF] TrackChanged({}) -> {:?}", position, self.state_tracker.get());
    }

    pub fn handle_eof(&self) {
        log::trace!("[DIAG-EOF] EOF");

        let repeat_mode = self.queue.repeat_mode();
        let current_idx = self.queue.current_index();
        let queue_len = self.queue.len();
        log::info!(
            "[DIAG-EOF] handle_eof: repeat_mode={:?} current_idx={:?} queue_len={}",
            repeat_mode,
            current_idx,
            queue_len
        );

        if self.try_recover_from_early_eof(current_idx) {
            return;
        }

        let intent = determine_advance_intent(&self.queue, repeat_mode);
        log::info!("[DIAG-EOF] Captured intent: {:?}", intent);

        if intent == AdvanceIntent::Repeat {
            log::info!("[DIAG-EOF] Repeat One: replaying current track");
            if let Some(current_idx) = self.queue.current_index() {
                let _ = self.play_position_or_go_idle(current_idx, "repeat-one replay");
                return;
            }
            if let Err(e) = self.playback.seek(0.0, "absolute") {
                log::error!("Failed to seek to start: {}", e);
            }
            if let Err(e) = self.playback.unpause() {
                log::error!("Failed to unpause: {}", e);
            }
            self.state_tracker.force_set(PlaybackState::Playing);
            return;
        }

        let from_position = self.queue.current_index().unwrap_or(0);
        self.state_tracker.force_set(PlaybackState::PendingAdvance {
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

        self.spawn_pending_advance_timeout();
    }

    fn try_recover_from_early_eof(&self, current_idx: Option<usize>) -> bool {
        let Some(idx) = current_idx else {
            return false;
        };

        let Ok(song) = self.queue.get_by_index(idx) else {
            return false;
        };
        let video_id = stable_track_id(&song.uri);
        if video_id.is_empty() {
            return false;
        }

        let position_secs = match self.playback.get_position() {
            Ok(pos) => pos,
            Err(err) => {
                log::debug!("[DIAG-EOF] Could not read time-pos for {}: {}", video_id, err);
                return false;
            }
        };
        let duration_secs = match self.playback.get_duration() {
            Ok(dur) => dur,
            Err(err) => {
                log::debug!("[DIAG-EOF] Could not read duration for {}: {}", video_id, err);
                return false;
            }
        };

        if !is_suspicious_eof(position_secs, duration_secs) {
            self.early_eof_counts.lock().remove(&video_id);
            return false;
        }

        let remaining_secs = duration_secs - position_secs;
        let mut attempts = self.early_eof_counts.lock();
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
            "[DIAG-EOF] Suspicious EOF for {} at {:.2}/{:.2}s (remaining {:.2}s), retrying current track with fresh URL (attempt {}/{})",
            video_id,
            position_secs,
            duration_secs,
            remaining_secs,
            attempt_no,
            EARLY_EOF_MAX_RECOVERY_ATTEMPTS
        );

        self.media_preparer.invalidate(&video_id);

        match self.play_position_internal_sync(idx) {
            Ok(()) => {
                if position_secs > 1.0 {
                    if let Err(e) = self.playback.seek(position_secs, "absolute") {
                        log::warn!(
                            "[DIAG-EOF] Failed to seek to {:.1}s after recovery: {}",
                            position_secs,
                            e
                        );
                    } else {
                        log::info!(
                            "[DIAG-EOF] Recovered {} at {:.1}s with fresh URL",
                            video_id,
                            position_secs
                        );
                    }
                }
                true
            }
            Err(err) => {
                log::warn!("[DIAG-EOF] Early-EOF recovery replay failed for {}: {}", video_id, err);
                false
            }
        }
    }

    pub(crate) fn spawn_pending_advance_timeout(&self) {
        let playback = Arc::clone(&self.playback);
        let queue = Arc::clone(&self.queue);
        let state_tracker = Arc::clone(&self.state_tracker);
        let media_preparer = Arc::clone(&self.media_preparer);
        let prefetch_manager = Arc::clone(&self.prefetch_manager);
        let play_position_sync = Arc::clone(&self.play_position_sync);
        let early_eof_counts = Arc::clone(&self.early_eof_counts);

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
            let timeout_handler = TrackEndedHandler {
                playback,
                queue,
                state_tracker,
                media_preparer,
                prefetch_manager,
                play_position_sync,
                early_eof_counts,
            };
            timeout_handler.handle_track_changed(position);
        });
    }

    fn execute_intent(&self, position: i32, intent: AdvanceIntent) -> bool {
        match intent {
            AdvanceIntent::Repeat => {
                if let Some(current_idx) = self.queue.current_index() {
                    let _ = self.play_position_or_go_idle(current_idx, "repeat intent replay");
                } else {
                    let _ = self.playback.seek(0.0, "absolute");
                    let _ = self.playback.unpause();
                    self.state_tracker.force_set(PlaybackState::Playing);
                }
                true
            }
            AdvanceIntent::Stop => {
                log::info!("[DIAG-EOF] Intent::Stop - end of queue");
                self.queue.set_current(None);
                self.state_tracker.force_set(PlaybackState::Idle);
                true
            }
            AdvanceIntent::Advance => {
                if position < 0 {
                    self.handle_end_of_window(self.queue.repeat_mode());
                    true
                } else {
                    self.handle_within_window_advance(position as usize)
                }
            }
        }
    }

    pub(crate) fn handle_end_of_window(&self, repeat_mode: RepeatMode) {
        log::info!("[DIAG-EOF] handle_end_of_window: start");
        let next_pos = self.queue.next_index();
        log::info!(
            "[DIAG-EOF] handle_end_of_window: current={:?} next_pos={:?} queue_len={}",
            self.queue.current_index(),
            next_pos,
            self.queue.len()
        );

        match next_pos {
            Some(pos) => {
                log::info!("[DIAG-EOF] End of prefetch window, loading next batch at {}", pos);
                let _ = self.play_position_or_go_idle(pos, "end-of-window reload");
            }
            None => {
                if repeat_mode == RepeatMode::All && self.queue.len() > 0 {
                    log::info!("[DIAG-EOF] Repeat All: looping back to start");
                    let _ = self.play_position_or_go_idle(0, "repeat-all wrap reload");
                } else {
                    log::info!("[DIAG-EOF] Reached end of queue, going idle");
                    self.queue.set_current(None);
                    self.state_tracker.force_set(PlaybackState::Idle);
                }
            }
        }
    }

    pub(crate) fn handle_within_window_advance(&self, mpv_pos: usize) -> bool {
        let Some(new_queue_pos) = self.queue.get_prefetched_at(mpv_pos) else {
            log::error!(
                "[DIAG-EOF] Prefetch lookup failed for mpv_pos={}, playback_base_index={}, prefetch_window={:?}; refusing unsafe fallback",
                mpv_pos,
                self.queue.playback_base_index(),
                self.queue.playback_window_state().prefetch_indices,
            );
            return false;
        };

        log::info!(
            "[DIAG-EOF] handle_within_window_advance: mpv_pos={} new_queue_pos={} queue_len={}",
            mpv_pos,
            new_queue_pos,
            self.queue.len()
        );

        if new_queue_pos >= self.queue.len() {
            log::warn!(
                "[DIAG-EOF] new_queue_pos {} out of bounds (len={})",
                new_queue_pos,
                self.queue.len()
            );
            return false;
        }

        log::info!(
            "[DIAG-EOF] MPV at playlist-pos {}, queue pos now {}",
            mpv_pos,
            new_queue_pos
        );
        self.queue.set_current(Some(new_queue_pos));
        self.state_tracker.force_set(PlaybackState::Playing);

        if let Ok(song) = self.queue.get_by_index(new_queue_pos) {
            let title = song
                .metadata
                .get("title")
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_else(|| song.uri.clone());
            let artist = song
                .metadata
                .get("artist")
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_default();
            let _ = self.playback.set_media_title(&title, &artist);
        }

        let prefetch_window_before_extend = self.queue.playback_window_state().prefetch_indices;
        let rollback_active_indices: Vec<usize> =
            prefetch_window_before_extend.iter().skip(mpv_pos).copied().collect();
        if let Some(next_idx) = self.queue.extend_prefetch_window() {
            let active_indices = self
                .queue
                .compute_prefetch_window(new_queue_pos, PREFETCH_WINDOW_SIZE);
            activate_playback_window(&self.media_preparer, &self.queue, &active_indices);

            if let Ok(song) = self.queue.get_by_index(next_idx) {
                let track_id = stable_track_id(&song.uri);
                if !track_id.is_empty()
                    && let Ok(url) = prepare_media_blocking(
                        &self.media_preparer,
                        &track_id,
                        crate::backends::youtube::media::PreloadTier::Eager,
                    )
                    .and_then(|prepared| {
                        build_runtime_mpv_input(&self.playback, &track_id, &prepared)
                    })
                {
                    if self.playback.playlist_append_input(&url).is_ok() {
                        log::debug!("Extended prefetch window with queue index {}", next_idx);
                    } else {
                        self.queue
                            .set_prefetch_indices(prefetch_window_before_extend.clone());
                        activate_playback_window(
                            &self.media_preparer,
                            &self.queue,
                            &rollback_active_indices,
                        );
                    }
                } else {
                    self.queue
                        .set_prefetch_indices(prefetch_window_before_extend.clone());
                    activate_playback_window(
                        &self.media_preparer,
                        &self.queue,
                        &rollback_active_indices,
                    );
                }
            } else {
                self.queue
                    .set_prefetch_indices(prefetch_window_before_extend.clone());
                activate_playback_window(&self.media_preparer, &self.queue, &rollback_active_indices);
            }
        }

        true
    }

    pub(crate) fn handle_playback_error(&self) {
        log::warn!("Playback error, selecting next track using playback order");

        let repeat_mode = self.queue.repeat_mode();
        let next_pos = match repeat_mode {
            RepeatMode::One => self.queue.current_index(),
            RepeatMode::Off => self.queue.next_index(),
            RepeatMode::All => self
                .queue
                .next_index()
                .or_else(|| (self.queue.len() > 0).then_some(0)),
        };

        if let Some(next_pos) = next_pos {
            let _ = self.play_position_or_go_idle(next_pos, "playback-error recovery");
        } else {
            self.queue.set_current(None);
            self.state_tracker.force_set(PlaybackState::Idle);
        }
    }

    fn play_position_internal_sync(&self, pos: usize) -> anyhow::Result<()> {
        match (self.play_position_sync)(pos) {
            ServerResponse::Ok => Ok(()),
            ServerResponse::Error(e) => Err(anyhow::anyhow!("{}", e)),
            _ => Ok(()),
        }
    }

    fn play_position_or_go_idle(&self, pos: usize, context: &str) -> bool {
        match self.play_position_internal_sync(pos) {
            Ok(()) => true,
            Err(err) => {
                log::warn!("{}: failed to load queue position {}: {}", context, pos, err);
                self.queue.set_current(None);
                self.state_tracker.force_set(PlaybackState::Idle);
                false
            }
        }
    }
}

fn activate_playback_window(
    media_preparer: &Arc<dyn crate::backends::youtube::media::MediaPreparer>,
    queue: &Arc<QueueService>,
    indices: &[usize],
) {
    let track_ids: Vec<String> = indices
        .iter()
        .filter_map(|&idx| queue.get_by_index(idx).ok())
        .map(|song| stable_track_id(&song.uri))
        .filter(|track_id| !track_id.is_empty())
        .collect();

    media_preparer.activate_playback_window(&track_ids);
}
