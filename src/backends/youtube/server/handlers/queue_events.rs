//! Event handlers for `PlayQueue` events (Layer 2 Bridge).
//!
//! This handler bridges `PlayQueue` state changes to MPV playlist operations.
//! Key responsibility: Keep MPV's playlist synchronized with queue order
//! changes.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{extract_video_id, stable_track_id};
use crate::{
    backends::youtube::{
        audio::MpvInput,
        media::{MediaPreparer, PreloadTier},
        server::orchestrator::PREFETCH_WINDOW_SIZE,
        server::playback_prepare::prepare_media_blocking,
        server::{
            playback_coordinator::PlaybackCoordinator, playback_horizon::ResolvedPlaybackHorizon,
        },
        services::{PlaybackService, QueueService},
    },
    shared::play_queue::{PlayQueue, QueueEvent, QueueId, RepeatMode},
};

pub struct QueueEventHandler {
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    play_queue: Arc<Mutex<PlayQueue>>,
    media_preparer: Option<Arc<dyn MediaPreparer>>,
    playback_coordinator: Option<Arc<Mutex<PlaybackCoordinator>>>,
    prefix_window_worker_kick: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl QueueEventHandler {
    #[must_use]
    pub fn new(
        playback: Arc<PlaybackService>,
        queue: Arc<QueueService>,
        play_queue: Arc<Mutex<PlayQueue>>,
    ) -> Self {
        Self {
            playback,
            queue,
            play_queue,
            media_preparer: None,
            playback_coordinator: None,
            prefix_window_worker_kick: None,
        }
    }

    pub fn with_media_preparer(mut self, preparer: Arc<dyn MediaPreparer>) -> Self {
        self.media_preparer = Some(preparer);
        self
    }

    pub fn with_playback_coordinator(
        mut self,
        coordinator: Arc<Mutex<PlaybackCoordinator>>,
    ) -> Self {
        self.playback_coordinator = Some(coordinator);
        self
    }

    pub fn with_prefix_window_worker_kick(mut self, kick: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.prefix_window_worker_kick = Some(kick);
        self
    }

    pub fn handle(&mut self, event: QueueEvent) {
        match event {
            QueueEvent::ItemsAdded { ids } => self.handle_items_added(&ids),
            QueueEvent::ItemsRemoved { ids } => self.handle_items_removed(&ids),
            QueueEvent::OrderChanged { play_order, current_id } => {
                self.handle_order_changed(&play_order, current_id);
            }
            QueueEvent::CurrentChanged { from, to } => self.handle_current_changed(from, to),
            QueueEvent::ModesChanged { shuffle, repeat } => {
                self.handle_modes_changed(shuffle, repeat);
            }
            QueueEvent::Cleared => self.handle_cleared(),
            QueueEvent::Stopped => self.handle_stopped(),
        }
    }

    fn handle_items_added(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsAdded: {ids:?}");

        let play_queue = self.play_queue.lock();
        let added_track_ids = added_track_ids(&play_queue, ids);
        let play_order = play_queue.get_play_order().to_vec();
        let current_id = play_queue.get_current_id();

        if let Some(coordinator) = &self.playback_coordinator {
            sync_coordinator_horizon(coordinator, &play_queue, &play_order, current_id);
        }
        self.kick_prefix_window_worker_if_started();

        let added_track_ids = filter_background_extract_track_ids(
            added_track_ids,
            self.playback_coordinator.as_ref(),
        );
        drop(play_queue);

        if added_track_ids.is_empty() && play_order.is_empty() {
            return;
        }

        if let Some(ref preparer) = self.media_preparer {
            if !added_track_ids.is_empty() {
                preparer.warm_many(&added_track_ids);
            }
        } else {
            log::warn!("QueueEventHandler missing media preparer; skipping queue warm");
        }
    }

    fn handle_items_removed(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsRemoved: {ids:?}");
    }

    fn handle_order_changed(&mut self, play_order: &[QueueId], current_id: Option<QueueId>) {
        log::debug!("QueueEvent::OrderChanged: {} items, current={current_id:?}", play_order.len());

        if let Some(coordinator) = &self.playback_coordinator {
            sync_coordinator_horizon(coordinator, &self.play_queue.lock(), play_order, current_id);
        }
        self.kick_prefix_window_worker_if_started();

        if self.queue.current_index().is_some() {
            log::debug!("Skipping QueueEvent::OrderChanged playback sync during active playback");
            return;
        }

        let mpv_playlist_len = match self.playback.get_playlist_count() {
            Ok(len) => len,
            Err(e) => {
                log::warn!("Failed to get MPV playlist count: {e}");
                return;
            }
        };

        for i in (1..mpv_playlist_len).rev() {
            if let Err(e) = self.playback.playlist_remove(i) {
                log::warn!("Failed to remove MPV playlist item {i}: {e}");
            }
        }

        let current_pos = current_id.and_then(|id| play_order.iter().position(|&x| x == id));

        if let Some(current_pos) = current_pos {
            let window_end = std::cmp::min(current_pos + PREFETCH_WINDOW_SIZE, play_order.len());

            for (offset, &id) in play_order[current_pos + 1..window_end].iter().enumerate() {
                let tier = if offset == 0 { PreloadTier::Gapless } else { PreloadTier::Eager };
                if let Some(input) = self.resolve_playback_input(id, tier) {
                    if let Err(e) = self.playback.playlist_append_input(&input) {
                        log::warn!("Failed to append track {id} to MPV playlist: {e}");
                    } else {
                        log::debug!("Appended track {id} to MPV playlist");
                    }
                }
            }
        } else if !play_order.is_empty() {
            let window_end = std::cmp::min(PREFETCH_WINDOW_SIZE, play_order.len());
            for (offset, &id) in play_order[..window_end].iter().enumerate() {
                let tier = match offset {
                    0 => PreloadTier::Immediate,
                    1 => PreloadTier::Gapless,
                    _ => PreloadTier::Eager,
                };
                if let Some(input) = self.resolve_playback_input(id, tier) {
                    if let Err(e) = self.playback.playlist_append_input(&input) {
                        log::warn!("Failed to append track {id} to MPV playlist: {e}");
                    }
                }
            }
        }
    }

    fn resolve_playback_input(&self, id: QueueId, tier: PreloadTier) -> Option<MpvInput> {
        let play_queue = self.play_queue.lock();
        let song = play_queue.get_song(id)?;
        let video_id = stable_track_id(&song.uri);
        if video_id.is_empty() {
            log::warn!("Song {id} has empty video_id");
            return None;
        }
        drop(play_queue);

        let Some(preparer) = self.media_preparer.as_ref() else {
            log::warn!(
                "QueueEventHandler missing media preparer; cannot resolve media for {video_id}"
            );
            return None;
        };

        prepare_media_blocking(preparer, &video_id, tier)
            .and_then(|prepared| self.playback.build_runtime_input(&video_id, &prepared))
            .map_err(|e| {
                log::warn!("Failed to prepare playback input for {video_id}: {e}");
                e
            })
            .ok()
    }

    fn handle_current_changed(&mut self, from: Option<QueueId>, to: Option<QueueId>) {
        log::debug!("QueueEvent::CurrentChanged: {from:?} -> {to:?}");
        if let Some(coordinator) = &self.playback_coordinator {
            let play_queue = self.play_queue.lock();
            sync_coordinator_horizon(coordinator, &play_queue, play_queue.get_play_order(), to);
        }
        self.kick_prefix_window_worker_if_started();
    }

    fn handle_modes_changed(&mut self, shuffle: bool, repeat: RepeatMode) {
        log::debug!("QueueEvent::ModesChanged: shuffle={shuffle}, repeat={repeat:?}");
    }

    fn handle_cleared(&mut self) {
        log::debug!("QueueEvent::Cleared");
        if let Some(coordinator) = &self.playback_coordinator {
            coordinator.lock().reset();
        }
        if let Some(ref preparer) = self.media_preparer {
            preparer.activate_playback_window(&[]);
        }
    }

    fn handle_stopped(&mut self) {
        log::debug!("QueueEvent::Stopped");
        if let Some(coordinator) = &self.playback_coordinator {
            coordinator.lock().reset();
        }
        if let Some(ref preparer) = self.media_preparer {
            preparer.activate_playback_window(&[]);
        }
    }

    fn kick_prefix_window_worker_if_started(&self) {
        let playback_started = self
            .playback_coordinator
            .as_ref()
            .map(|coordinator| coordinator.lock().playback_started())
            .unwrap_or(false);
        if playback_started && let Some(kick) = &self.prefix_window_worker_kick {
            kick();
        }
    }
}

fn added_track_ids(play_queue: &PlayQueue, ids: &[QueueId]) -> Vec<String> {
    ids.iter()
        .filter_map(|&id| play_queue.get_song(id))
        .filter_map(|song| extract_video_id(&song.uri))
        .collect()
}

fn sync_coordinator_horizon(
    coordinator: &Arc<Mutex<PlaybackCoordinator>>,
    play_queue: &PlayQueue,
    play_order: &[QueueId],
    current_id: Option<QueueId>,
) {
    let observed_current_track = current_id
        .and_then(|id| play_queue.get_song(id))
        .map(|song| stable_track_id(&song.uri))
        .filter(|track_id| !track_id.is_empty());

    let anchor_track = {
        let coordinator = coordinator.lock();
        if coordinator.should_preserve_pending_current_track(observed_current_track.as_deref()) {
            coordinator.current_track_id().or_else(|| observed_current_track.clone())
        } else {
            observed_current_track.clone().or_else(|| coordinator.current_track_id())
        }
    };

    let horizon = ResolvedPlaybackHorizon::from_play_queue_track_id(
        play_queue,
        play_order,
        anchor_track.as_deref(),
    );

    coordinator.lock().sync_with_queue_observation(observed_current_track, horizon);
}

fn filter_background_extract_track_ids(
    track_ids: Vec<String>,
    coordinator: Option<&Arc<Mutex<PlaybackCoordinator>>>,
) -> Vec<String> {
    let Some(coordinator) = coordinator else {
        return track_ids;
    };

    track_ids
        .into_iter()
        .filter(|track_id| coordinator.lock().should_accept_queue_extract_result(track_id))
        .collect()
}

#[cfg(test)]
fn prefetch_playback_window(
    preparer: &Arc<dyn MediaPreparer>,
    play_queue: &PlayQueue,
    play_order: &[QueueId],
    current_id: Option<QueueId>,
    current_queue_index: Option<usize>,
) {
    let start_pos = playback_window_start_pos(play_order, current_id, current_queue_index);
    if start_pos >= play_order.len() {
        return;
    }

    let window_end = std::cmp::min(start_pos + PREFETCH_WINDOW_SIZE, play_order.len());
    let window = &play_order[start_pos..window_end];
    let track_ids = normalized_window_track_ids(play_queue, window);
    if track_ids.is_empty() {
        return;
    }

    preparer.activate_playback_window(&track_ids);
    for (index, track_id) in track_ids.iter().enumerate() {
        let tier = match index {
            0 => PreloadTier::Immediate,
            1 => PreloadTier::Gapless,
            _ => PreloadTier::Eager,
        };
        preparer.prefetch(track_id, tier);
    }
}

#[cfg(test)]
fn playback_window_start_pos(
    play_order: &[QueueId],
    current_id: Option<QueueId>,
    current_queue_index: Option<usize>,
) -> usize {
    if let Some(current_pos) = current_queue_index.filter(|&pos| pos < play_order.len()) {
        return current_pos;
    }

    current_id.and_then(|id| play_order.iter().position(|&x| x == id)).unwrap_or(0)
}

#[cfg(test)]
fn normalized_window_track_ids(play_queue: &PlayQueue, window: &[QueueId]) -> Vec<String> {
    window
        .iter()
        .filter_map(|&id| play_queue.get_song(id).map(|s| stable_track_id(&s.uri)))
        .filter(|track_id| !track_id.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::backends::youtube::{
        audio::AudioDeliveryPlanner,
        config::{AudioDeliveryMode, ExtractorType},
        server::test_support::{MpvTestGuard, RecordingMediaPreparer, acquire_mpv_test_guard},
        url_resolver::UrlResolver,
    };
    use crate::domain::Song;

    fn setup_playback_services() -> (MpvTestGuard, TempDir, Arc<PlaybackService>, Arc<QueueService>)
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
                AudioDeliveryPlanner.plan(AudioDeliveryMode::Direct),
                None,
            )
            .unwrap(),
        );

        let queue = Arc::new(QueueService::new());
        (mpv_guard, temp_dir, playback, queue)
    }

    fn test_song(uri: &str) -> Song {
        Song { uri: uri.to_string(), ..Song::default() }
    }

    #[test]
    fn added_track_ids_extract_supported_shapes() {
        let mut play_queue = PlayQueue::new();
        let ids = [
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "video123".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song {
                    uri: "https://music.youtube.com/watch?v=video456".to_string(),
                    ..Song::default()
                },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "https://youtu.be/video789".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
        ];

        assert_eq!(
            added_track_ids(&play_queue, &ids),
            vec!["video123".to_string(), "video456".to_string(), "video789".to_string(),]
        );
    }

    #[test]
    fn normalized_window_track_ids_uses_stable_ids() {
        let mut play_queue = PlayQueue::new();
        let ids = [
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song {
                    uri: "https://www.youtube.com/watch?v=video123".to_string(),
                    ..Song::default()
                },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "https://youtu.be/video456".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "youtube://video789".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
        ];

        assert_eq!(
            normalized_window_track_ids(&play_queue, &ids),
            vec!["video123".to_string(), "video456".to_string(), "video789".to_string(),]
        );
    }

    #[test]
    fn prefetch_playback_window_prefetches_initial_window_when_idle() {
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();

        let mut play_queue = PlayQueue::new();
        play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
            song: Song {
                uri: "https://music.youtube.com/watch?v=video123".to_string(),
                ..Song::default()
            },
        });
        play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
            song: Song { uri: "https://youtu.be/video456".to_string(), ..Song::default() },
        });
        play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
            song: Song { uri: "youtube://video789".to_string(), ..Song::default() },
        });
        play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
            song: Song { uri: "video101".to_string(), ..Song::default() },
        });

        let play_order = play_queue.get_play_order().to_vec();
        prefetch_playback_window(&preparer, &play_queue, &play_order, None, None);

        assert_eq!(
            recording.activated_windows.lock().as_slice(),
            &[vec!["video123".to_string(), "video456".to_string(), "video789".to_string(),]]
        );
        assert_eq!(
            recording.prefetched.lock().as_slice(),
            &[
                ("video123".to_string(), PreloadTier::Immediate),
                ("video456".to_string(), PreloadTier::Gapless),
                ("video789".to_string(), PreloadTier::Eager),
            ]
        );
    }

    #[test]
    fn prefetch_playback_window_uses_queue_current_index_when_playing() {
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();

        let mut play_queue = PlayQueue::new();
        let ids = [
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song {
                    uri: "https://music.youtube.com/watch?v=video123".to_string(),
                    ..Song::default()
                },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "https://youtu.be/video456".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "youtube://video789".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
            match play_queue.apply(crate::shared::play_queue::QueueCommand::Add {
                song: Song { uri: "video101".to_string(), ..Song::default() },
            })[0]
            {
                QueueEvent::ItemsAdded { ref ids } => ids[0],
                _ => unreachable!(),
            },
        ];

        let play_order = play_queue.get_play_order().to_vec();
        prefetch_playback_window(&preparer, &play_queue, &play_order, Some(ids[0]), Some(2));

        assert_eq!(
            recording.activated_windows.lock().as_slice(),
            &[vec!["video789".to_string(), "video101".to_string()]]
        );
        assert_eq!(
            recording.prefetched.lock().as_slice(),
            &[
                ("video789".to_string(), PreloadTier::Immediate),
                ("video101".to_string(), PreloadTier::Gapless),
            ]
        );
    }

    #[test]
    fn handle_items_added_only_warms_tracks_during_active_playback() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();

        queue.add(test_song("video000"), None);
        queue.add(test_song("video111"), None);
        queue.add(test_song("video222"), None);
        queue.set_current(Some(0));

        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") });
        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video111") });
        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video222") });

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer);

        queue.add(test_song("video999"), Some(1));
        let events = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video999") });
        for event in events {
            handler.handle(event);
        }

        assert_eq!(recording.warmed_batches.lock().as_slice(), &[vec!["video999".to_string()]]);
        assert!(recording.activated_windows.lock().is_empty());
        assert!(recording.prefetched.lock().is_empty());
    }

    #[test]
    fn filter_background_extract_drops_current_immediate_track_but_keeps_others() {
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));
        coordinator.lock().begin_immediate_play("video000");

        assert_eq!(
            filter_background_extract_track_ids(
                vec!["video000".to_string(), "video111".to_string(), "video222".to_string()],
                Some(&coordinator)
            ),
            vec!["video111".to_string(), "video222".to_string()]
        );
    }

    #[test]
    fn handle_items_added_updates_coordinator_horizon_without_prefetch_window_side_effects() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));

        let current_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        play_queue.lock().apply(crate::shared::play_queue::QueueCommand::Play { id: current_id });
        coordinator.lock().begin_immediate_play("video000");

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_playback_coordinator(Arc::clone(&coordinator));

        let added_event = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video999") });
        for event in added_event {
            handler.handle(event);
        }

        assert_eq!(coordinator.lock().snapshot().resolved_horizon, vec!["video000", "video999"]);
        assert_eq!(recording.warmed_batches.lock().as_slice(), &[vec!["video999".to_string()]]);
        assert!(recording.activated_windows.lock().is_empty());
        assert!(recording.prefetched.lock().is_empty());
    }

    #[test]
    fn handle_items_added_repairs_stale_current_track_before_recomputing_horizon() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));

        let current_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        play_queue.lock().apply(crate::shared::play_queue::QueueCommand::Play { id: current_id });
        coordinator.lock().begin_immediate_play("stale-current");
        coordinator.lock().mark_bytes_started("stale-current");

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_playback_coordinator(Arc::clone(&coordinator));

        let added_event = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video999") });
        for event in added_event {
            handler.handle(event);
        }

        let snapshot = coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("video000"));
        assert_eq!(snapshot.next_three_window, vec!["video999"]);
        assert!(!snapshot.next_three_window.contains(&"video000".to_string()));
    }

    #[test]
    fn handle_stopped_resets_coordinator_state() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));

        coordinator.lock().queue_changed(ResolvedPlaybackHorizon::new(vec![
            "video000".to_string(),
            "video111".to_string(),
        ]));
        coordinator.lock().begin_immediate_play("video000");
        coordinator.lock().mark_bytes_started("video000");
        assert_eq!(coordinator.lock().claim_next_prefix_job().as_deref(), Some("video111"));

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_playback_coordinator(Arc::clone(&coordinator));

        handler.handle(QueueEvent::Stopped);

        let snapshot = coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track, None);
        assert_eq!(snapshot.current_owner, None);
        assert!(!snapshot.playback_started);
        assert!(snapshot.next_three_window.is_empty());
        assert!(snapshot.active_prefix_job.is_none());
    }

    #[test]
    fn handle_items_added_preserves_inflight_immediate_play_when_play_queue_current_is_stale() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));

        let stale_current_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video111") });
        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video222") });
        play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Play { id: stale_current_id });

        coordinator.lock().begin_immediate_play("video111");

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_playback_coordinator(Arc::clone(&coordinator));

        let added_event = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video333") });
        for event in added_event {
            handler.handle(event);
        }

        let snapshot = coordinator.lock().snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("video111"));
        assert_eq!(
            snapshot.current_owner,
            Some(
                crate::backends::youtube::server::playback_coordinator::TrackOwner::ImmediateRelay
            )
        );
        assert!(!snapshot.playback_started);
        assert_eq!(snapshot.resolved_horizon, vec!["video111", "video222", "video333", "video000"]);
    }
}
