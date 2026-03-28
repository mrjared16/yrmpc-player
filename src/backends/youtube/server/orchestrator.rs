//! Orchestrator for YouTube backend server.
//!
//! Contains the complex business logic for:
//! - Prefetch window management (3-track rolling window)
//! - Track-ended handling (auto-advance, repeat modes)
//! - Play position logic with MPRIS updates
//!
//! This module is used by both the main server and the internal event
//! processor.

use std::{sync::Arc, thread};

use anyhow::{Context, Result};
use crossbeam::channel::Sender;
use parking_lot::Mutex;

use crate::backends::youtube::{
    audio::MpvInput,
    media::{MediaPreparer, PreloadTier, PreparedMedia},
    protocol::{ServerResponse, SongData},
    server::handlers::stable_track_id,
    server::playback_coordinator::PlaybackCoordinator,
    server::playback_horizon::ResolvedPlaybackHorizon,
    server::playback_prepare::{
        build_current_runtime_input_with_direct_fallback, build_runtime_mpv_input,
        prepare_media_blocking,
    },
    server::prefetch_manager::PrefetchManager,
    server::track_ended::TrackEndedHandler,
    services::{
        AdvanceIntent, PlaybackService, PlaybackState, PlaybackStateTracker, QueueService,
        RepeatMode, playback_service::RuntimeInputRoute,
    },
};

/// Prefetch window size for gapless playback
pub const PREFETCH_WINDOW_SIZE: usize = 3;

/// Threshold for triggering prefetch (seconds remaining)
fn tier_for_window_offset(offset: usize) -> PreloadTier {
    match offset {
        0 => PreloadTier::Immediate,
        1 => PreloadTier::Gapless,
        _ => PreloadTier::Eager,
    }
}

fn activate_playback_window(
    media_preparer: &Arc<dyn MediaPreparer>,
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

fn resolved_horizon_track_ids(queue: &Arc<QueueService>, start_idx: usize) -> Vec<String> {
    queue
        .compute_prefetch_window(start_idx, queue.len())
        .into_iter()
        .filter_map(|idx| queue.get_by_index(idx).ok())
        .map(|song| stable_track_id(&song.uri))
        .filter(|track_id| !track_id.is_empty())
        .collect()
}

// ============================================================================
// Orchestrator struct — wraps the 4-param clump + former global statics
// ============================================================================

pub struct Orchestrator {
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    state_tracker: Arc<PlaybackStateTracker>,
    media_preparer: Arc<dyn MediaPreparer>,
    coordinator: Arc<Mutex<PlaybackCoordinator>>,
    prefix_window_kick: Sender<()>,
    prefetch: Arc<PrefetchManager>,
    track_ended: Arc<TrackEndedHandler>,
}

impl Orchestrator {
    pub fn new(
        playback: Arc<PlaybackService>,
        queue: Arc<QueueService>,
        state_tracker: Arc<PlaybackStateTracker>,
        media_preparer: Arc<dyn MediaPreparer>,
    ) -> Self {
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));
        let prefetch =
            Arc::new(PrefetchManager::new(Arc::clone(&queue), Arc::clone(&media_preparer)));
        let (prefix_window_kick, prefix_window_rx) = crossbeam::channel::bounded(1);
        let playback_for_track_ended = Arc::clone(&playback);
        let queue_for_track_ended = Arc::clone(&queue);
        let state_for_track_ended = Arc::clone(&state_tracker);
        let preparer_for_track_ended = Arc::clone(&media_preparer);
        let coordinator_for_track_ended = Arc::clone(&coordinator);
        let play_position_sync: Arc<dyn Fn(usize) -> ServerResponse + Send + Sync> =
            Arc::new(move |pos| {
                play_position_sync_with_services(
                    &playback_for_track_ended,
                    &queue_for_track_ended,
                    &state_for_track_ended,
                    &preparer_for_track_ended,
                    &coordinator_for_track_ended,
                    pos,
                )
            });
        let track_ended = Arc::new(TrackEndedHandler::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            Arc::clone(&state_tracker),
            Arc::clone(&media_preparer),
            Arc::clone(&prefetch),
            play_position_sync,
        ));
        spawn_prefix_window_worker(
            Arc::clone(&media_preparer),
            Arc::clone(&coordinator),
            prefix_window_rx,
        );

        Self {
            playback,
            queue,
            state_tracker,
            media_preparer,
            coordinator,
            prefix_window_kick,
            prefetch,
            track_ended,
        }
    }

    // -- Public API --

    pub async fn play_position(&self, pos: usize) -> ServerResponse {
        self.queue.ensure_shuffle_order(pos);
        self.play_position_sync_impl(pos)
    }

    pub fn play_position_sync(&self, pos: usize) -> ServerResponse {
        self.queue.ensure_shuffle_order(pos);
        self.play_position_sync_impl(pos)
    }

    pub fn next_track(&self) -> ServerResponse {
        match self.queue.next_index() {
            Some(idx) => self.play_position_sync(idx),
            None => ServerResponse::Error("No next track".into()),
        }
    }

    pub fn previous_track(&self) -> ServerResponse {
        match self.queue.previous_index() {
            Some(0) => match self.playback.seek(0.0, "absolute") {
                Ok(_) => {
                    self.state_tracker.force_set(PlaybackState::Playing);
                    ServerResponse::Ok
                }
                Err(e) => ServerResponse::Error(e.to_string()),
            },
            Some(idx) => self.play_position_sync(idx),
            None => ServerResponse::Error("No previous track".into()),
        }
    }

    pub fn play_id(&self, id: u32) -> ServerResponse {
        match self.queue.find_index_by_id(id) {
            Some(pos) => self.play_position_sync(pos),
            None => ServerResponse::Error("Song not found".into()),
        }
    }

    pub fn get_current_song(&self) -> ServerResponse {
        if let Some(idx) = self.queue.current_index() {
            if let Ok(song) = self.queue.get_by_index(idx) {
                return ServerResponse::Song(Some(SongData::from(song)));
            }
        }
        ServerResponse::Song(None)
    }

    pub fn get_playlist(&self) -> ServerResponse {
        let songs = self.queue.get_all();
        ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
    }

    pub fn handle_track_ended(&self, reason: &str) {
        self.track_ended.handle_track_ended(reason);
    }

    pub fn handle_track_changed(&self, position: i32) {
        self.track_ended.handle_track_changed(position);
        self.sync_coordinator_after_track_changed();
    }

    pub fn handle_playback_started(&self) {
        self.handle_playback_started_for_current_track();
    }

    pub fn handle_playback_started_for_current_track(&self) {
        let mut coordinator = self.coordinator.lock();
        let Some(track_id) = coordinator.current_track_id() else {
            return;
        };

        if !coordinator.playback_started() {
            coordinator.mark_bytes_started(&track_id);
            drop(coordinator);
            self.kick_prefix_window_worker();
        }
    }

    pub fn prefetch_upcoming(&self) {
        self.kick_prefix_window_worker();
    }

    pub fn kick_prefix_window_worker(&self) {
        match self.prefix_window_kick.try_send(()) {
            Ok(()) | Err(crossbeam::channel::TrySendError::Full(())) => {}
            Err(crossbeam::channel::TrySendError::Disconnected(())) => {}
        }
    }

    #[cfg(test)]
    fn handle_eof(&self) {
        self.track_ended.handle_eof();
    }

    #[cfg(test)]
    fn handle_playback_error(&self) {
        self.track_ended.handle_playback_error();
    }

    #[cfg(test)]
    fn handle_end_of_window(&self, repeat_mode: RepeatMode) {
        self.track_ended.handle_end_of_window(repeat_mode);
    }

    #[cfg(test)]
    fn handle_within_window_advance(&self, mpv_pos: usize) -> bool {
        let advanced = self.track_ended.handle_within_window_advance(mpv_pos);
        if advanced {
            self.sync_coordinator_after_track_changed();
        }
        advanced
    }

    #[cfg(test)]
    fn spawn_pending_advance_timeout(&self) {
        self.track_ended.spawn_pending_advance_timeout();
    }

    // -- Internal methods --

    fn play_position_sync_impl(&self, pos: usize) -> ServerResponse {
        play_position_sync_with_services(
            &self.playback,
            &self.queue,
            &self.state_tracker,
            &self.media_preparer,
            &self.coordinator,
            pos,
        )
    }

    fn sync_coordinator_after_track_changed(&self) {
        let Some(current_idx) = self.queue.current_index() else {
            return;
        };

        let current_track = self
            .queue
            .get_by_index(current_idx)
            .ok()
            .map(|song| stable_track_id(&song.uri))
            .filter(|track_id| !track_id.is_empty());
        let new_horizon =
            ResolvedPlaybackHorizon::from_queue_service(self.queue.as_ref(), current_idx);

        let mut coordinator = self.coordinator.lock();
        if coordinator.should_preserve_pending_current_track(current_track.as_deref()) {
            return;
        }

        coordinator.sync_with_queue_observation(current_track, new_horizon);

        if coordinator.playback_started() {
            drop(coordinator);
            self.kick_prefix_window_worker();
        }
    }

    pub fn reconcile_active_window_after_queue_mutation(&self) -> Result<()> {
        let current_idx = match self.queue.current_index() {
            Some(idx) => idx,
            None => return Ok(()),
        };

        let state = self.state_tracker.get();
        if matches!(state, PlaybackState::Idle | PlaybackState::Stopped) {
            return Ok(());
        }

        let current_mpv_pos = self
            .playback
            .get_playlist_pos()
            .context("read MPV playlist position during queue reconciliation")?;
        if current_mpv_pos < 0 {
            return Ok(());
        }

        let current_mpv_pos = usize::try_from(current_mpv_pos)
            .context("convert MPV playlist position during queue reconciliation")?;

        let previous_window = self.queue.playback_window_state();
        let previous_current_window_pos = previous_window
            .current_idx
            .and_then(|window_current_idx| {
                previous_window.prefetch_indices.iter().position(|&idx| idx == window_current_idx)
            })
            .unwrap_or(current_mpv_pos);
        let existing_tail: Vec<usize> = previous_window
            .prefetch_indices
            .iter()
            .skip(previous_current_window_pos.saturating_add(1))
            .copied()
            .collect();

        for idx in (0..current_mpv_pos).rev() {
            self.playback.playlist_remove(idx).with_context(|| {
                format!("remove stale MPV prefix item {idx} during reconciliation")
            })?;
        }

        let active_indices = self.queue.compute_prefetch_window(current_idx, PREFETCH_WINDOW_SIZE);
        let desired_tail: Vec<usize> = active_indices.iter().skip(1).copied().collect();
        let unchanged_tail_len = existing_tail
            .iter()
            .zip(desired_tail.iter())
            .take_while(|(existing, desired)| existing == desired)
            .count();

        let playlist_count = self
            .playback
            .get_playlist_count()
            .context("read MPV playlist count during queue reconciliation")?;
        for idx in ((unchanged_tail_len + 1)..playlist_count).rev() {
            self.playback.playlist_remove(idx).with_context(|| {
                format!("remove stale MPV tail item {idx} during reconciliation")
            })?;
        }

        self.queue.set_playback_window_state(
            Some(current_idx),
            current_idx,
            active_indices.clone(),
        );
        activate_playback_window(&self.media_preparer, &self.queue, &active_indices);
        let playback_started = {
            let mut coordinator = self.coordinator.lock();
            coordinator.queue_changed(ResolvedPlaybackHorizon::from_queue_service(
                self.queue.as_ref(),
                current_idx,
            ));
            coordinator.playback_started()
        };
        if playback_started {
            self.kick_prefix_window_worker();
        }

        for (offset, &queue_idx) in active_indices.iter().enumerate().skip(1 + unchanged_tail_len) {
            let song = self
                .queue
                .get_by_index(queue_idx)
                .with_context(|| format!("read queue item {queue_idx} during reconciliation"))?;
            let track_id = stable_track_id(&song.uri);
            if track_id.is_empty() {
                continue;
            }

            let input = prepare_media_blocking(
                &self.media_preparer,
                &track_id,
                tier_for_window_offset(offset),
            )
            .and_then(|prepared| build_runtime_mpv_input(&self.playback, &track_id, &prepared))
            .with_context(|| format!("prepare track {track_id} during queue reconciliation"))?;
            self.playback
                .playlist_append_input(&input)
                .with_context(|| format!("append track {track_id} during queue reconciliation"))?;
        }

        Ok(())
    }

    // -- Accessors for callers that need inner services --

    pub fn playback(&self) -> &Arc<PlaybackService> {
        &self.playback
    }

    pub fn queue(&self) -> &Arc<QueueService> {
        &self.queue
    }

    pub fn state_tracker(&self) -> &Arc<PlaybackStateTracker> {
        &self.state_tracker
    }

    pub fn media_preparer(&self) -> &Arc<dyn MediaPreparer> {
        &self.media_preparer
    }

    pub fn coordinator(&self) -> &Arc<Mutex<PlaybackCoordinator>> {
        &self.coordinator
    }
}

fn spawn_prefix_window_worker(
    media_preparer: Arc<dyn MediaPreparer>,
    coordinator: Arc<Mutex<PlaybackCoordinator>>,
    prefix_window_rx: crossbeam::channel::Receiver<()>,
) {
    thread::spawn(move || {
        let mut last_activated_window: Option<Vec<String>> = None;

        while prefix_window_rx.recv().is_ok() {
            loop {
                let active_window = coordinator.lock().next_three_window();
                activate_window_if_changed(
                    media_preparer.as_ref(),
                    &mut last_activated_window,
                    active_window,
                );

                let Some(track_id) = coordinator.lock().claim_next_prefix_job() else {
                    break;
                };

                let should_prepare = {
                    let mut coordinator = coordinator.lock();
                    coordinator.revalidate_claimed_prefix_job(&track_id)
                };
                if !should_prepare {
                    continue;
                }

                let result =
                    prepare_media_blocking(&media_preparer, &track_id, PreloadTier::Background);
                let active_window = {
                    let mut coordinator = coordinator.lock();
                    match result {
                        Ok(
                            PreparedMedia::StagedPrefix { .. } | PreparedMedia::LocalFile { .. },
                        ) => {
                            coordinator.finish_prefix_job(&track_id);
                        }
                        Ok(other) => {
                            log::warn!(
                                "prefix window worker expected staged background prefix for {}, got {:?}",
                                track_id,
                                other
                            );
                            coordinator.fail_prefix_job(&track_id);
                        }
                        Err(err) => {
                            log::warn!("prefix window worker failed for {}: {}", track_id, err);
                            coordinator.fail_prefix_job(&track_id);
                        }
                    }
                    coordinator.next_three_window()
                };
                activate_window_if_changed(
                    media_preparer.as_ref(),
                    &mut last_activated_window,
                    active_window,
                );
            }
        }
    });
}

fn activate_window_if_changed(
    media_preparer: &dyn MediaPreparer,
    last_activated_window: &mut Option<Vec<String>>,
    active_window: Vec<String>,
) {
    if last_activated_window.as_ref() == Some(&active_window) {
        return;
    }

    media_preparer.activate_playback_window(&active_window);
    *last_activated_window = Some(active_window);
}

fn play_position_sync_with_services(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    media_preparer: &Arc<dyn MediaPreparer>,
    coordinator: &Arc<Mutex<PlaybackCoordinator>>,
    pos: usize,
) -> ServerResponse {
    log::info!("play_position_sync for pos={} (uses MediaPreparer)", pos);

    if let Err(e) = playback.stop() {
        log::debug!("stop() returned error (may be idle): {}", e);
    }
    state_tracker.force_set(PlaybackState::Idle);

    if let Err(e) = playback.playlist_clear() {
        log::warn!("Failed to clear MPV buffer: {}", e);
    }

    let plan = match build_playback_plan(queue, pos) {
        Ok(plan) => plan,
        Err(e) => return ServerResponse::Error(e.to_string()),
    };

    let Some(track) = plan.tracks.first() else {
        state_tracker.force_set(PlaybackState::Idle);
        return ServerResponse::Error("No playable tracks found".to_string());
    };

    {
        let mut coordinator = coordinator.lock();
        coordinator.queue_changed(ResolvedPlaybackHorizon::from_queue_service(queue.as_ref(), pos));
        coordinator.begin_immediate_play(track.track_id.clone());
    }

    let mut appended_prefetch_indices: Vec<usize> = Vec::new();

    let first_url_metadata = match prepare_media_blocking(
        media_preparer,
        &track.track_id,
        track.tier,
    )
    .and_then(|prepared| {
        build_current_runtime_input_with_direct_fallback(playback, &track.track_id, &prepared)
    }) {
        Ok(decision) => {
            if matches!(decision.route, RuntimeInputRoute::DirectFallback) {
                coordinator.lock().swap_current_track_to_direct_fallback(&track.track_id);
            }

            if let Err(e) = playback.playlist_append_input(&decision.input) {
                log::error!("Failed to append to playlist: {}", e);
                return ServerResponse::Error(format!("Failed to append stream: {}", e));
            }

            log::debug!("Prepared track (queue pos {}) via MediaPreparer", track.queue_index);
            appended_prefetch_indices.push(track.queue_index);
            (track.title.clone(), track.artist.clone())
        }
        Err(e) => {
            log::error!("Failed to prepare media for {}: {}", track.track_id, e);
            return ServerResponse::Error(format!("Failed to prepare stream: {}", e));
        }
    };

    if appended_prefetch_indices.is_empty() {
        state_tracker.force_set(PlaybackState::Idle);
        return ServerResponse::Error("Failed to append any playable track".to_string());
    }

    queue.set_playback_window_state(Some(pos), pos, appended_prefetch_indices.clone());
    activate_playback_window(media_preparer, queue, &appended_prefetch_indices);

    if appended_prefetch_indices.len() < plan.prefetch_indices.len() {
        log::warn!(
            "playback window truncated after append failures: requested={} appended={}",
            plan.prefetch_indices.len(),
            appended_prefetch_indices.len()
        );
    }

    if let Err(e) = playback.set_media_title(&first_url_metadata.0, &first_url_metadata.1) {
        log::warn!("Failed to set media title: {}", e);
    }

    match playback.playlist_play_index(0) {
        Ok(_) => {
            log::info!("Playing position {} (prefetched {} tracks)", pos, plan.tracks.len());
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

// ============================================================================
// PlaybackPlan — pure data structure describing what to play
// ============================================================================

/// A track entry in the playback plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTrack {
    /// Queue index of the track
    pub queue_index: usize,
    pub track_id: String,
    /// Video/track URI
    pub uri: String,
    /// Title for MPRIS metadata
    pub title: String,
    /// Artist for MPRIS metadata
    pub artist: String,
    /// Preload tier for this track
    pub tier: PreloadTier,
}

/// Pure description of what play_position should do, without side effects.
#[derive(Debug, Clone)]
pub struct PlaybackPlan {
    /// Queue position to start playing from
    pub position: usize,
    /// Ordered list of tracks to load into MPV's playlist
    pub tracks: Vec<PlannedTrack>,
    /// The prefetch window indices (stored for state tracking)
    pub prefetch_indices: Vec<usize>,
}

/// Pure function: builds a PlaybackPlan from queue state.
pub fn build_playback_plan(queue: &Arc<QueueService>, pos: usize) -> Result<PlaybackPlan> {
    let queue_len = queue.len();
    if pos >= queue_len {
        return Err(anyhow::anyhow!("Position {} out of bounds (queue len={})", pos, queue_len));
    }

    let prefetch_indices = queue.compute_prefetch_window(pos, 1);

    let tracks: Vec<PlannedTrack> = prefetch_indices
        .iter()
        .enumerate()
        .filter_map(|(i, &idx)| {
            queue.get_by_index(idx).ok().map(|song| {
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
                PlannedTrack {
                    queue_index: idx,
                    track_id: stable_track_id(&song.uri),
                    uri: song.uri.clone(),
                    title,
                    artist,
                    tier: tier_for_window_offset(i),
                }
            })
        })
        .collect();

    if tracks.is_empty() {
        return Err(anyhow::anyhow!("No playable tracks found at position {}", pos));
    }

    Ok(PlaybackPlan { position: pos, tracks, prefetch_indices })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        backends::youtube::{
            audio::{AudioDeliveryPlanner, PreparedMediaInputAdapter},
            config::{AudioDeliveryMode, ExtractorType},
            media::RelayRuntime,
            server::test_support::{MpvTestGuard, RecordingMediaPreparer, acquire_mpv_test_guard},
            url_resolver::UrlResolver,
        },
        domain::Song,
    };

    fn setup_test_services()
    -> (MpvTestGuard, TempDir, Arc<PlaybackService>, Arc<QueueService>, Arc<PlaybackStateTracker>)
    {
        setup_test_services_with_mode(AudioDeliveryMode::Direct, None)
    }

    fn setup_test_services_with_mode(
        mode: AudioDeliveryMode,
        relay_runtime: Option<Arc<RelayRuntime>>,
    ) -> (MpvTestGuard, TempDir, Arc<PlaybackService>, Arc<QueueService>, Arc<PlaybackStateTracker>)
    {
        let mpv_guard = acquire_mpv_test_guard();
        let temp_dir = TempDir::new().unwrap();
        let socket = temp_dir.path().join("test-mpv.sock");

        let url_resolver = Arc::new(UrlResolver::new(ExtractorType::default()));
        let playback = Arc::new(
            PlaybackService::new(
                &socket,
                url_resolver,
                None,
                AudioDeliveryPlanner.plan(mode),
                relay_runtime,
            )
            .unwrap(),
        );
        let queue = Arc::new(QueueService::new());
        let state_tracker = Arc::new(PlaybackStateTracker::new());

        (mpv_guard, temp_dir, playback, queue, state_tracker)
    }

    /// Create an Orchestrator from test services with a stub preparer
    fn setup_orchestrator() -> (MpvTestGuard, TempDir, Orchestrator) {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        let orch = Orchestrator::new(playback, queue, state_tracker, stub_media_preparer());
        (mpv_guard, temp_dir, orch)
    }

    /// Create an Orchestrator with a custom RecordingMediaPreparer
    fn setup_orchestrator_with_preparer()
    -> (MpvTestGuard, TempDir, Orchestrator, Arc<RecordingMediaPreparer>) {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();
        let orch = Orchestrator::new(playback, queue, state_tracker, media_preparer);
        (mpv_guard, temp_dir, orch, recording)
    }

    fn wait_for_prepared_entries(
        recording: &Arc<RecordingMediaPreparer>,
        expected_len: usize,
    ) -> Vec<(String, PreloadTier)> {
        for _ in 0..50 {
            let prepared = recording.prepared.lock().clone();
            if prepared.len() >= expected_len {
                return prepared;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        recording.prepared.lock().clone()
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

    struct StreamAndCacheMediaPreparer;

    #[async_trait]
    impl MediaPreparer for StreamAndCacheMediaPreparer {
        async fn prepare(&self, track_id: &str, _tier: PreloadTier) -> Result<PreparedMedia> {
            Ok(PreparedMedia::StreamAndCache {
                url: format!("https://example.invalid/{track_id}"),
                content_length: 4096,
                prefix_path: PathBuf::from(format!("/tmp/{track_id}.webm")),
                prefix_size: 1024,
            })
        }

        fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}
    }

    struct SelectiveFailMediaPreparer {
        fail_ids: HashSet<String>,
    }

    #[async_trait]
    impl MediaPreparer for SelectiveFailMediaPreparer {
        async fn prepare(&self, track_id: &str, _tier: PreloadTier) -> Result<PreparedMedia> {
            if self.fail_ids.contains(track_id) {
                anyhow::bail!("forced prepare failure for {track_id}");
            }

            Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
        }

        fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}
    }

    #[derive(Default)]
    struct SelectiveRecordingMediaPreparer {
        fail_ids: HashSet<String>,
        activated_windows: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl MediaPreparer for SelectiveRecordingMediaPreparer {
        async fn prepare(&self, track_id: &str, _tier: PreloadTier) -> Result<PreparedMedia> {
            if self.fail_ids.contains(track_id) {
                anyhow::bail!("forced prepare failure for {track_id}");
            }

            Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
        }

        fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}

        fn activate_playback_window(&self, track_ids: &[String]) {
            self.activated_windows.lock().push(track_ids.to_vec());
        }
    }

    // =========================================================================
    // Pure utility tests (no Orchestrator needed)
    // =========================================================================

    #[test]
    fn sync_and_async_window_tier_mapping_matches() {
        assert_eq!(tier_for_window_offset(0), PreloadTier::Immediate);
        assert_eq!(tier_for_window_offset(1), PreloadTier::Gapless);
        assert_eq!(tier_for_window_offset(2), PreloadTier::Eager);
        assert_eq!(tier_for_window_offset(5), PreloadTier::Eager);
    }

    #[test]
    fn build_playback_plan_does_not_mutate_prefetch_state() {
        let queue = Arc::new(QueueService::new());
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);

        let plan = build_playback_plan(&queue, 0).unwrap();

        assert_eq!(plan.prefetch_indices, vec![0]);
        assert_eq!(queue.get_prefetched_at(0), None);
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
    fn play_position_sync_normalizes_full_youtube_urls() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        orch.queue().add(test_song("https://www.youtube.com/watch?v=video123"), None);
        orch.queue().add(test_song("https://youtu.be/video456"), None);
        orch.queue().add(test_song("https://www.youtube.com/shorts/video789"), None);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        assert_eq!(
            *recording.prepared.lock(),
            vec![("video123".to_string(), PreloadTier::Immediate)]
        );
        assert!(recording.prefetched.lock().is_empty());
        assert_eq!(*recording.activated_windows.lock(), vec![vec!["video123".to_string()]]);
    }

    #[test]
    fn play_position_sync_defers_future_window_until_playback_started_hook() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        orch.queue().add(test_song("video123"), None);
        orch.queue().add(test_song("video456"), None);
        orch.queue().add(test_song("video789"), None);
        orch.queue().add(test_song("video999"), None);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        assert!(orch.coordinator.lock().snapshot().next_three_window.is_empty());
        assert_eq!(
            *recording.prepared.lock(),
            vec![("video123".to_string(), PreloadTier::Immediate)]
        );
        assert!(recording.prefetched.lock().is_empty());

        orch.handle_playback_started_for_current_track();

        let prepared = wait_for_prepared_entries(&recording, 4);

        assert_eq!(
            orch.coordinator.lock().snapshot().next_three_window,
            vec!["video456", "video789", "video999"]
        );
        assert_eq!(
            prepared,
            vec![
                ("video123".to_string(), PreloadTier::Immediate),
                ("video456".to_string(), PreloadTier::Background),
                ("video789".to_string(), PreloadTier::Background),
                ("video999".to_string(), PreloadTier::Background),
            ]
        );
        assert!(recording.activated_windows.lock().contains(&vec![
            "video456".to_string(),
            "video789".to_string(),
            "video999".to_string()
        ]));
    }

    #[test]
    fn runtime_builder_uses_concat_source_for_staged_prefix() {
        let (_mpv_guard, _temp_dir, playback, _queue, _state_tracker) = setup_test_services();

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
        assert_eq!(input.mpv_args, PreparedMediaInputAdapter::protocol_whitelist_args());
    }

    #[test]
    fn runtime_builder_preserves_direct_fallback_input() {
        let (_mpv_guard, _temp_dir, playback, _queue, _state_tracker) = setup_test_services();

        let prepared = PreparedMedia::Direct { url: "https://example.invalid/direct".to_string() };

        let input = build_runtime_mpv_input(&playback, "v1", &prepared).unwrap();

        assert_eq!(input.url, "https://example.invalid/direct");
        assert!(input.mpv_args.is_empty());
    }

    #[test]
    fn play_position_sync_swaps_current_owner_to_direct_fallback_when_relay_is_unavailable() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) =
            setup_test_services_with_mode(AudioDeliveryMode::Relay, None);
        let media_preparer: Arc<dyn MediaPreparer> = Arc::new(StreamAndCacheMediaPreparer);
        let orch = Orchestrator::new(playback, queue.clone(), state_tracker, media_preparer);

        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);
        queue.add(test_song("song-3"), None);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        let snapshot = orch.coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("song-0"));
        assert_eq!(
            snapshot.current_owner,
            Some(
                crate::backends::youtube::server::playback_coordinator::TrackOwner::DirectFallback
            )
        );
        assert!(!snapshot.playback_started);
        assert!(snapshot.next_three_window.is_empty());
        assert!(!orch.coordinator.lock().should_accept_queue_extract_result("song-0"));

        orch.handle_playback_started_for_current_track();

        let snapshot = orch.coordinator.lock().snapshot();
        assert_eq!(
            snapshot.current_owner,
            Some(
                crate::backends::youtube::server::playback_coordinator::TrackOwner::DirectFallback
            )
        );
        assert_eq!(snapshot.track_states.get("song-0"), Some(&crate::backends::youtube::server::playback_coordinator::TrackJobState::PlayingDirect));
        assert_eq!(snapshot.next_three_window, vec!["song-1", "song-2", "song-3"]);
        assert!(!orch.coordinator.lock().should_accept_queue_extract_result("song-0"));

        drop(temp_dir);
        drop(mpv_guard);
    }

    #[test]
    fn suspicious_eof_detects_early_cutoff() {
        assert!(crate::backends::youtube::server::track_ended::is_suspicious_eof(210.6, 236.98));
    }

    #[test]
    fn suspicious_eof_ignores_near_end() {
        assert!(!crate::backends::youtube::server::track_ended::is_suspicious_eof(228.0, 236.98));
    }

    #[test]
    fn suspicious_eof_ignores_short_tracks() {
        assert!(!crate::backends::youtube::server::track_ended::is_suspicious_eof(20.0, 45.0));
    }

    // =========================================================================
    // Orchestrator struct tests
    // =========================================================================

    #[test]
    fn eof_advances_to_next_track() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().add(test_song("Song 2"), None);
        orch.queue().set_current(Some(0));
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1]);

        orch.handle_eof();
        orch.handle_track_changed(1);

        assert_eq!(orch.queue().current_index(), Some(1));
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn track_changed_during_immediate_prepare_does_not_restore_previous_current_track() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().set_current(Some(0));

        {
            let mut coordinator = orch.coordinator.lock();
            coordinator.queue_changed(ResolvedPlaybackHorizon::from_queue_service(
                orch.queue().as_ref(),
                0,
            ));
            coordinator.begin_immediate_play("song-1");
        }

        orch.handle_track_changed(-1);

        let snapshot = orch.coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("song-1"));
        assert_eq!(
            snapshot.current_owner,
            Some(
                crate::backends::youtube::server::playback_coordinator::TrackOwner::ImmediateRelay
            )
        );
        assert!(!snapshot.playback_started);
    }

    #[test]
    fn track_changed_during_direct_fallback_prepare_does_not_restore_previous_current_track() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().set_current(Some(0));

        {
            let mut coordinator = orch.coordinator.lock();
            coordinator.queue_changed(ResolvedPlaybackHorizon::from_queue_service(
                orch.queue().as_ref(),
                0,
            ));
            coordinator.begin_immediate_play("song-1");
            assert!(coordinator.swap_current_track_to_direct_fallback("song-1"));
        }

        orch.handle_track_changed(-1);

        let snapshot = orch.coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("song-1"));
        assert_eq!(
            snapshot.current_owner,
            Some(
                crate::backends::youtube::server::playback_coordinator::TrackOwner::DirectFallback
            )
        );
        assert!(!snapshot.playback_started);
    }

    #[test]
    fn eof_enters_pending_advance_before_track_changed_confirmation() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().add(test_song("Song 2"), None);
        orch.queue().set_current(Some(0));

        orch.handle_eof();

        match orch.state_tracker().get() {
            PlaybackState::PendingAdvance { from_position, intent, .. } => {
                assert_eq!(from_position, 0);
                assert_eq!(intent, AdvanceIntent::Advance);
            }
            other => panic!("expected pending advance, got {other:?}"),
        }

        orch.handle_track_changed(1);
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn pending_advance_timeout_updates_original_orchestrator_state() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().set_current(Some(0));
        orch.prefetch.mark_triggered_for_test("song-0".to_string());
        orch.state_tracker().force_set(PlaybackState::PendingAdvance {
            since: Instant::now() - Duration::from_secs(3),
            from_position: 0,
            intent: AdvanceIntent::Advance,
        });

        orch.spawn_pending_advance_timeout();
        std::thread::sleep(Duration::from_millis(2200));

        assert_eq!(orch.queue().current_index(), Some(1));
        assert!(
            !orch.prefetch.is_triggered_for_test("song-0"),
            "timeout fallback should mutate the original orchestrator state"
        );
    }

    #[test]
    fn eof_with_repeat_one_replays_current() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().set_current(Some(0));
        orch.queue().set_repeat_mode(RepeatMode::One);

        orch.handle_eof();

        assert_eq!(orch.queue().current_index(), Some(0));
    }

    #[test]
    fn eof_at_end_with_repeat_all_loops() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().add(test_song("Song 2"), None);
        orch.queue().set_current(Some(1)); // Last song
        orch.queue().set_repeat_mode(RepeatMode::All);

        orch.handle_eof();
        orch.handle_track_changed(-1);

        assert_eq!(orch.queue().current_index(), Some(0)); // Looped
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn eof_at_end_without_repeat_goes_idle() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().set_current(Some(0));
        orch.queue().set_repeat_mode(RepeatMode::Off);

        orch.handle_eof();
        orch.handle_track_changed(-1);

        assert_eq!(orch.queue().current_index(), None);
        assert_eq!(orch.state_tracker().get(), PlaybackState::Idle);
        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn playback_error_uses_playback_order_repeat_all_wraps_to_start() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().set_current(Some(1));
        orch.queue().set_repeat_mode(RepeatMode::All);

        orch.handle_playback_error();

        assert_eq!(orch.queue().current_index(), Some(0));
    }

    #[test]
    fn end_of_window_reload_failure_goes_idle() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.set_current(Some(0));

        let fail_ids = HashSet::from(["song-1".to_string()]);
        let media_preparer: Arc<dyn MediaPreparer> =
            Arc::new(SelectiveFailMediaPreparer { fail_ids });
        let orch =
            Orchestrator::new(playback, queue.clone(), state_tracker.clone(), media_preparer);

        orch.handle_end_of_window(RepeatMode::Off);

        assert_eq!(queue.current_index(), None);
        assert_eq!(state_tracker.get(), PlaybackState::Idle);

        drop(temp_dir);
        drop(mpv_guard);
    }

    #[test]
    fn within_window_advance_missing_prefetch_mapping_does_not_fallback_to_base_plus_pos() {
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().add(test_song("song-2"), None);
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(1);
        orch.queue().set_prefetch_indices(vec![0]);

        let advanced = orch.handle_within_window_advance(1);

        assert!(!advanced);
        assert_eq!(orch.queue().current_index(), Some(0));
    }

    #[test]
    fn play_position_sync_tracks_only_successfully_appended_window_indices() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);

        let fail_ids = HashSet::from(["song-1".to_string(), "song-2".to_string()]);
        let media_preparer: Arc<dyn MediaPreparer> =
            Arc::new(SelectiveFailMediaPreparer { fail_ids });
        let orch = Orchestrator::new(playback, queue.clone(), state_tracker, media_preparer);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        let playback_window = queue.playback_window_state();
        assert_eq!(playback_window.current_idx, Some(0));
        assert_eq!(playback_window.playback_base_index, 0);
        assert_eq!(playback_window.prefetch_indices, vec![0]);

        drop(temp_dir);
        drop(mpv_guard);
    }

    #[test]
    fn play_position_sync_does_not_leave_sparse_prefetch_window_after_middle_failure() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);

        let fail_ids = HashSet::from(["song-1".to_string()]);
        let media_preparer: Arc<dyn MediaPreparer> =
            Arc::new(SelectiveFailMediaPreparer { fail_ids });
        let orch = Orchestrator::new(playback, queue.clone(), state_tracker, media_preparer);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        let playback_window = queue.playback_window_state();
        assert_eq!(playback_window.current_idx, Some(0));
        assert_eq!(playback_window.playback_base_index, 0);
        assert_eq!(playback_window.prefetch_indices, vec![0]);

        drop(temp_dir);
        drop(mpv_guard);
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
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();

        // Add 5 songs
        for i in 0..5 {
            orch.queue().add(test_song(&format!("Song {}", i)), None);
        }

        // Enable shuffle BEFORE starting playback
        orch.queue().set_shuffle_enabled(true);

        // Start playing from position 0
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);

        // KEY: Call build_prefetch_window to populate prefetch_indices
        // This is what play_position does in production
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);

        // The first track should be position 0 (where we started)
        assert_eq!(prefetch_indices[0], 0, "First track should be starting position");

        // With shuffle enabled, subsequent tracks should be from shuffle order
        // They might be sequential (20% chance with 5 items), so we test that
        // the prefetch lookup works correctly instead of asserting randomness

        // Simulate: MPV auto-advanced within window (mpv_pos=1)
        orch.handle_within_window_advance(1);

        // After advance, current should match what was at prefetch_indices[1]
        let current = orch.queue().current_index().expect("Should have current");

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
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();

        // Add 5 songs
        for i in 0..5 {
            orch.queue().add(test_song(&format!("Song {}", i)), None);
        }

        // Enable shuffle
        orch.queue().set_shuffle_enabled(true);

        // Start from position 0
        // play_position will build prefetch window [0, 1, 2] SEQUENTIALLY
        // but with shuffle enabled, it SHOULD build [0, shuffle[1], shuffle[2]]
        let _ = orch.play_position_sync(0);

        // After play_position, check what base_index was set to
        let base = orch.queue().playback_base_index();
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
        let next_shuffle = orch.queue().next_index();

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
        let (_mpv_guard, _temp_dir, orch) = setup_orchestrator();

        // Add 3 songs (exactly PREFETCH_WINDOW_SIZE)
        orch.queue().add(test_song("Song 0"), None);
        orch.queue().add(test_song("Song 1"), None);
        orch.queue().add(test_song("Song 2"), None);

        // Enable Repeat All
        orch.queue().set_repeat_mode(RepeatMode::All);

        // Start at position 0
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);

        // KEY: Build prefetch window - this is what play_position does
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2], "Initial prefetch window");

        // Simulate: MPV at last track in window (mpv_pos=2)
        // This is Song 2, which is also the last in queue
        orch.handle_within_window_advance(2);

        // Current should be 2
        assert_eq!(orch.queue().current_index(), Some(2));

        assert_eq!(orch.queue().get_prefetched_at(3), Some(0));
    }

    #[test]
    fn playback_started_marks_playback_started_and_kicks_prefix_window_worker() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("youtube://song-1"), None);
        assert!(matches!(orch.play_position_sync(0), ServerResponse::Ok));

        orch.handle_playback_started();

        let prepared = wait_for_prepared_entries(&recording, 2);
        assert_eq!(prepared[0], ("song-0".to_string(), PreloadTier::Immediate));
        assert_eq!(prepared[1], ("song-1".to_string(), PreloadTier::Background));
        assert_eq!(orch.coordinator().lock().next_three_window(), vec!["song-1".to_string()]);
        assert!(orch.coordinator().lock().playback_started());

        let activated_windows = recording.activated_windows.lock().clone();
        assert_eq!(activated_windows, vec![vec!["song-0".to_string()], vec!["song-1".to_string()]]);
    }

    #[test]
    fn playback_started_debounces_repeated_start_trigger_for_same_track() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();
        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        assert!(matches!(orch.play_position_sync(0), ServerResponse::Ok));

        orch.handle_playback_started();
        let _ = wait_for_prepared_entries(&recording, 2);
        orch.handle_playback_started();
        std::thread::sleep(Duration::from_millis(50));

        let prepared = recording.prepared.lock().clone();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[1], ("song-1".to_string(), PreloadTier::Background));
        assert_eq!(
            recording.activated_windows.lock().clone(),
            vec![vec!["song-0".to_string()], vec!["song-1".to_string()]]
        );
    }

    #[test]
    fn within_window_advance_refreshes_active_playback_window() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().add(test_song("song-2"), None);
        orch.queue().add(test_song("song-3"), None);
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        orch.handle_within_window_advance(1);

        assert_eq!(orch.queue().current_index(), Some(1));
        assert_eq!(orch.coordinator().lock().current_track_id().as_deref(), Some("song-1"));
        assert!(orch.coordinator().lock().next_three_window().is_empty());
        assert_eq!(
            recording.activated_windows.lock().clone(),
            vec![vec!["song-1".to_string(), "song-2".to_string(), "song-3".to_string(),]]
        );
        assert_eq!(
            recording.prepared.lock().clone(),
            vec![("song-3".to_string(), PreloadTier::Eager)]
        );
    }

    #[test]
    fn consecutive_within_window_advances_keep_absolute_mpv_slot_mapping() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        for idx in 0..5 {
            orch.queue().add(test_song(&format!("song-{idx}")), None);
        }
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        assert!(orch.handle_within_window_advance(1));
        assert_eq!(orch.queue().current_index(), Some(1));
        assert_eq!(orch.coordinator().lock().current_track_id().as_deref(), Some("song-1"));
        assert_eq!(orch.queue().get_prefetched_at(2), Some(2));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(3));

        assert!(orch.handle_within_window_advance(2));
        assert_eq!(orch.queue().current_index(), Some(2));
        assert_eq!(orch.coordinator().lock().current_track_id().as_deref(), Some("song-2"));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(3));
        assert_eq!(orch.queue().get_prefetched_at(4), Some(4));
        assert_eq!(
            recording.activated_windows.lock().clone(),
            vec![
                vec!["song-1".to_string(), "song-2".to_string(), "song-3".to_string()],
                vec!["song-2".to_string(), "song-3".to_string(), "song-4".to_string()],
            ]
        );
    }

    #[test]
    fn within_window_advance_repeat_all_keeps_absolute_mapping_through_wrap() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        for idx in 0..3 {
            orch.queue().add(test_song(&format!("song-{idx}")), None);
        }
        orch.queue().set_repeat_mode(RepeatMode::All);
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        assert!(orch.handle_within_window_advance(1));
        assert!(orch.handle_within_window_advance(2));
        assert!(orch.handle_within_window_advance(3));

        assert_eq!(orch.queue().current_index(), Some(0));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(0));
        assert_eq!(orch.queue().get_prefetched_at(4), Some(1));
        assert_eq!(
            recording.activated_windows.lock().clone(),
            vec![
                vec!["song-1".to_string(), "song-2".to_string(), "song-0".to_string()],
                vec!["song-2".to_string(), "song-0".to_string(), "song-1".to_string()],
                vec!["song-0".to_string(), "song-1".to_string(), "song-2".to_string()],
            ]
        );
    }

    #[test]
    fn track_changed_preserves_absolute_mapping_across_consecutive_advances() {
        let (_mpv_guard, _temp_dir, orch, _recording) = setup_orchestrator_with_preparer();

        for idx in 0..5 {
            orch.queue().add(test_song(&format!("song-{idx}")), None);
        }
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        orch.handle_eof();
        orch.handle_track_changed(1);
        assert_eq!(orch.queue().current_index(), Some(1));
        assert_eq!(orch.queue().get_prefetched_at(2), Some(2));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(3));

        orch.handle_eof();
        orch.handle_track_changed(2);
        assert_eq!(orch.queue().current_index(), Some(2));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(3));
        assert_eq!(orch.queue().get_prefetched_at(4), Some(4));

        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn track_changed_repeat_all_preserves_absolute_mapping_through_wrap() {
        let (_mpv_guard, _temp_dir, orch, _recording) = setup_orchestrator_with_preparer();

        for idx in 0..3 {
            orch.queue().add(test_song(&format!("song-{idx}")), None);
        }
        orch.queue().set_repeat_mode(RepeatMode::All);
        orch.queue().set_current(Some(0));
        orch.queue().set_playback_base_index(0);
        let prefetch_indices = orch.queue().build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        orch.handle_eof();
        orch.handle_track_changed(1);
        orch.handle_eof();
        orch.handle_track_changed(2);
        orch.handle_eof();
        orch.handle_track_changed(3);

        assert_eq!(orch.queue().current_index(), Some(0));
        assert_eq!(orch.queue().get_prefetched_at(3), Some(0));
        assert_eq!(orch.queue().get_prefetched_at(4), Some(1));

        std::thread::sleep(Duration::from_millis(2100));
    }

    #[test]
    fn within_window_advance_does_not_claim_unappended_tail_slot() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);
        queue.add(test_song("song-3"), None);

        let fail_ids = HashSet::from(["song-3".to_string()]);
        let media_preparer: Arc<dyn MediaPreparer> =
            Arc::new(SelectiveFailMediaPreparer { fail_ids });
        let orch = Orchestrator::new(playback, queue.clone(), state_tracker, media_preparer);

        queue.set_current(Some(0));
        queue.set_playback_base_index(0);
        let prefetch_indices = queue.build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        assert!(orch.handle_within_window_advance(1));
        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(queue.get_prefetched_at(2), Some(2));
        assert_eq!(queue.get_prefetched_at(3), None);

        drop(temp_dir);
        drop(mpv_guard);
    }

    #[test]
    fn within_window_advance_rolls_back_active_window_when_tail_prepare_fails() {
        let (mpv_guard, temp_dir, playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("song-0"), None);
        queue.add(test_song("song-1"), None);
        queue.add(test_song("song-2"), None);
        queue.add(test_song("song-3"), None);

        let media_preparer = Arc::new(SelectiveRecordingMediaPreparer {
            fail_ids: HashSet::from(["song-3".to_string()]),
            ..Default::default()
        });
        let orch =
            Orchestrator::new(playback, queue.clone(), state_tracker, media_preparer.clone());

        queue.set_current(Some(0));
        queue.set_playback_base_index(0);
        let prefetch_indices = queue.build_prefetch_window(0, 3);
        assert_eq!(prefetch_indices, vec![0, 1, 2]);

        assert!(orch.handle_within_window_advance(1));
        assert_eq!(queue.get_prefetched_at(3), None);
        assert_eq!(
            media_preparer.activated_windows.lock().clone(),
            vec![
                vec!["song-1".to_string(), "song-2".to_string(), "song-3".to_string()],
                vec!["song-1".to_string(), "song-2".to_string()],
            ]
        );

        drop(temp_dir);
        drop(mpv_guard);
    }

    #[test]
    fn queue_mutation_active_window_reconciles_through_orchestrator() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);
        orch.queue().add(test_song("song-2"), None);
        orch.queue().add(test_song("song-3"), None);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        recording.prepared.lock().clear();
        recording.prefetched.lock().clear();
        recording.activated_windows.lock().clear();

        orch.queue().add(test_song("song-new"), Some(1));
        orch.reconcile_active_window_after_queue_mutation().unwrap();

        assert_eq!(orch.queue().playback_base_index(), 0);
        assert_eq!(orch.queue().get_prefetched_at(0), Some(0));
        assert_eq!(orch.queue().get_prefetched_at(1), Some(1));
        assert_eq!(orch.queue().get_prefetched_at(2), Some(2));
        assert_eq!(
            recording.activated_windows.lock().clone(),
            vec![vec!["song-0".to_string(), "song-new".to_string(), "song-1".to_string(),]]
        );
        assert_eq!(
            recording.prepared.lock().clone(),
            vec![
                ("song-new".to_string(), PreloadTier::Gapless),
                ("song-1".to_string(), PreloadTier::Eager),
            ]
        );
    }

    #[test]
    fn queue_append_reconciliation_only_prepares_new_tail_entries() {
        let (_mpv_guard, _temp_dir, orch, recording) = setup_orchestrator_with_preparer();

        orch.queue().add(test_song("song-0"), None);
        orch.queue().add(test_song("song-1"), None);

        let response = orch.play_position_sync(0);
        assert!(matches!(response, ServerResponse::Ok));

        orch.reconcile_active_window_after_queue_mutation().unwrap();

        recording.prepared.lock().clear();
        recording.prefetched.lock().clear();
        recording.activated_windows.lock().clear();

        orch.queue().add(test_song("song-2"), None);
        orch.reconcile_active_window_after_queue_mutation().unwrap();

        assert_eq!(orch.queue().playback_base_index(), 0);
        assert_eq!(
            recording.prepared.lock().clone(),
            vec![("song-2".to_string(), PreloadTier::Eager)]
        );
    }
}
