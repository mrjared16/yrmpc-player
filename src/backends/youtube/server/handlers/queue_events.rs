//! Event handlers for `PlayQueue` events (Layer 2 Bridge).
//!
//! This handler bridges `PlayQueue` state changes into playback planning
//! updates. It must stay non-blocking so immediate startup remains owned by
//! orchestrator-driven playback.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{extract_video_id, stable_track_id};
use crate::{
    backends::youtube::{
        media::{MediaPreparer, PreloadTier},
        server::orchestrator::PREFETCH_WINDOW_SIZE,
        server::{
            playback_coordinator::PlaybackCoordinator, playback_horizon::ResolvedPlaybackHorizon,
        },
        services::{PlaybackService, QueueService},
    },
    shared::play_queue::{PlayQueue, QueueEvent, QueueId, RepeatMode},
};

pub struct QueueEventHandler {
    _playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    play_queue: Arc<Mutex<PlayQueue>>,
    _media_preparer: Option<Arc<dyn MediaPreparer>>,
    playback_coordinator: Option<Arc<Mutex<PlaybackCoordinator>>>,
    plan_changed: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl QueueEventHandler {
    #[must_use]
    pub fn new(
        playback: Arc<PlaybackService>,
        queue: Arc<QueueService>,
        play_queue: Arc<Mutex<PlayQueue>>,
    ) -> Self {
        Self {
            _playback: playback,
            queue,
            play_queue,
            _media_preparer: None,
            playback_coordinator: None,
            plan_changed: None,
        }
    }

    pub fn with_media_preparer(mut self, preparer: Arc<dyn MediaPreparer>) -> Self {
        self._media_preparer = Some(preparer);
        self
    }

    pub fn with_playback_coordinator(
        mut self,
        coordinator: Arc<Mutex<PlaybackCoordinator>>,
    ) -> Self {
        self.playback_coordinator = Some(coordinator);
        self
    }

    pub fn with_plan_changed(mut self, publish: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.plan_changed = Some(publish);
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
        let play_order = play_queue.get_play_order().to_vec();
        let current_id = play_queue.get_current_id();

        if let Some(coordinator) = &self.playback_coordinator {
            sync_coordinator_horizon(coordinator, &play_queue, &play_order, current_id);
        }
        self.publish_plan_changed();
        drop(play_queue);

        if play_order.is_empty() {
            return;
        }
    }

    fn handle_items_removed(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsRemoved: {ids:?}");

        let play_queue = self.play_queue.lock();
        let play_order = play_queue.get_play_order().to_vec();
        let current_id = play_queue.get_current_id();

        if let Some(coordinator) = &self.playback_coordinator {
            sync_coordinator_horizon(coordinator, &play_queue, &play_order, current_id);
        }
        self.publish_plan_changed();
    }

    fn handle_order_changed(&mut self, play_order: &[QueueId], current_id: Option<QueueId>) {
        log::debug!("QueueEvent::OrderChanged: {} items, current={current_id:?}", play_order.len());

        if let Some(coordinator) = &self.playback_coordinator {
            sync_coordinator_horizon(coordinator, &self.play_queue.lock(), play_order, current_id);
        }
        self.publish_plan_changed();
    }

    fn handle_current_changed(&mut self, from: Option<QueueId>, to: Option<QueueId>) {
        log::debug!("QueueEvent::CurrentChanged: {from:?} -> {to:?}");
        if let Some(coordinator) = &self.playback_coordinator {
            let play_queue = self.play_queue.lock();
            sync_coordinator_horizon(coordinator, &play_queue, play_queue.get_play_order(), to);
        }
        self.publish_plan_changed();
    }

    fn handle_modes_changed(&mut self, shuffle: bool, repeat: RepeatMode) {
        log::debug!("QueueEvent::ModesChanged: shuffle={shuffle}, repeat={repeat:?}");
    }

    fn handle_cleared(&mut self) {
        log::debug!("QueueEvent::Cleared");
        if let Some(coordinator) = &self.playback_coordinator {
            coordinator.lock().reset();
        }
        self.publish_plan_changed();
        if self.plan_changed.is_none()
            && let Some(ref preparer) = self._media_preparer
        {
            let empty: &[String] = &[];
            preparer.activate_playback_window(empty);
        }
    }

    fn handle_stopped(&mut self) {
        log::debug!("QueueEvent::Stopped");
        if let Some(coordinator) = &self.playback_coordinator {
            coordinator.lock().reset();
        }
        self.publish_plan_changed();
        if self.plan_changed.is_none()
            && let Some(ref preparer) = self._media_preparer
        {
            let empty: &[String] = &[];
            preparer.activate_playback_window(empty);
        }
    }

    fn publish_plan_changed(&self) {
        if let Some(publish) = &self.plan_changed {
            publish();
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

    let queue_track_ids =
        ResolvedPlaybackHorizon::from_play_queue_track_id(play_queue, play_order, None)
            .track_ids()
            .to_vec();

    coordinator.lock().sync_with_queue_observation(
        observed_current_track,
        horizon,
        queue_track_ids,
    );
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::backends::youtube::{
        audio::AudioDeliveryPlanner,
        config::{AudioDeliveryMode, BackgroundExtractMode, ExtractorType},
        media::MediaPreparationPlan,
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
    fn handle_items_added_only_updates_policy_during_active_playback() {
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

        assert!(recording.warmed_batches.lock().is_empty());
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
        assert!(recording.warmed_batches.lock().is_empty());
        assert!(recording.activated_windows.lock().is_empty());
        assert!(recording.prefetched.lock().is_empty());
    }

    #[test]
    fn handle_items_added_publishes_plan_changed_even_before_playback_started() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let notifications = Arc::new(AtomicUsize::new(0));

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_plan_changed({
                    let notifications = Arc::clone(&notifications);
                    Arc::new(move || {
                        notifications.fetch_add(1, Ordering::SeqCst);
                    })
                });

        let added_event = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video999") });
        for event in added_event {
            handler.handle(event);
        }

        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn handle_order_changed_before_playback_started_has_no_media_side_effects() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();

        let notifications = Arc::new(AtomicUsize::new(0));
        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_plan_changed({
                    let notifications = Arc::clone(&notifications);
                    Arc::new(move || {
                        notifications.fetch_add(1, Ordering::SeqCst);
                    })
                });

        let first_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        let second_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video111") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };

        handler.handle(QueueEvent::OrderChanged {
            play_order: vec![first_id, second_id],
            current_id: None,
        });

        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert!(recording.prepared.lock().is_empty());
        assert!(recording.prefetched.lock().is_empty());
        assert!(recording.warmed.lock().is_empty());
        assert!(recording.warmed_batches.lock().is_empty());
        assert!(recording.activated_windows.lock().is_empty());
    }

    #[test]
    fn handle_items_removed_recomputes_horizon_and_publishes_plan_changed() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));
        let notifications = Arc::new(AtomicUsize::new(0));

        let current_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video000") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        let removed_id = match play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Add { song: test_song("video111") })[0]
        {
            QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        play_queue.lock().apply(crate::shared::play_queue::QueueCommand::Play { id: current_id });

        coordinator.lock().begin_immediate_play("video000");
        coordinator.lock().mark_bytes_started("video000");
        coordinator.lock().queue_changed(
            ResolvedPlaybackHorizon::new(vec!["video000".to_string(), "video111".to_string()]),
            vec!["video000".to_string(), "video111".to_string()],
        );

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_playback_coordinator(Arc::clone(&coordinator))
                .with_plan_changed({
                    let notifications = Arc::clone(&notifications);
                    Arc::new(move || {
                        notifications.fetch_add(1, Ordering::SeqCst);
                    })
                });

        let removed_events = play_queue
            .lock()
            .apply(crate::shared::play_queue::QueueCommand::Remove { id: removed_id });
        for event in removed_events {
            handler.handle(event);
        }

        let snapshot = coordinator.lock().snapshot();
        assert_eq!(snapshot.resolved_horizon, vec!["video000"]);
        assert!(snapshot.extract_scope.is_empty());
        assert!(snapshot.prefix_window.is_empty());
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
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
        coordinator.lock().queue_changed(
            ResolvedPlaybackHorizon::new(vec!["video000".to_string()]),
            vec!["video000".to_string()],
        );
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
        assert_eq!(snapshot.extract_scope, vec!["video999"]);
        assert_eq!(snapshot.prefix_window, vec!["video999"]);
        assert!(!snapshot.extract_scope.contains(&"video000".to_string()));
    }

    #[test]
    fn handle_stopped_resets_coordinator_state() {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::default()));

        coordinator.lock().queue_changed(
            ResolvedPlaybackHorizon::new(vec!["video000".to_string(), "video111".to_string()]),
            vec!["video000".to_string(), "video111".to_string()],
        );
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
        assert!(snapshot.extract_scope.is_empty());
        assert!(snapshot.prefix_window.is_empty());
        assert!(snapshot.active_prefix_job.is_none());
    }

    fn media_plan_from_coordinator(
        coordinator: &PlaybackCoordinator,
        background_extract_mode: BackgroundExtractMode,
    ) -> MediaPreparationPlan {
        let plan = coordinator.preparation_plan();
        let mut active_window = Vec::new();
        if let Some(current_track) = plan.current_track {
            active_window.push(current_track);
        }
        active_window.extend(plan.prefix_window.iter().cloned());

        MediaPreparationPlan {
            background_extract_mode,
            active_window,
            prefix_targets: plan.prefix_window,
            extract_scope_generation: plan.extract_scope_generation,
            extract_scope: plan.extract_scope,
        }
    }

    fn assert_reset_event_clears_media_via_plan(event: QueueEvent) {
        let (_mpv_guard, _temp_dir, playback, queue) = setup_playback_services();
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let recording = Arc::new(RecordingMediaPreparer::fail_on_prepare());
        let preparer: Arc<dyn MediaPreparer> = recording.clone();
        let coordinator =
            Arc::new(Mutex::new(PlaybackCoordinator::new(BackgroundExtractMode::Balanced, 2)));

        coordinator.lock().queue_changed(
            ResolvedPlaybackHorizon::new(vec!["video000".to_string(), "video111".to_string()]),
            vec!["video000".to_string(), "video111".to_string()],
        );
        coordinator.lock().begin_immediate_play("video000");
        coordinator.lock().mark_bytes_started("video000");
        recording.apply_plan(media_plan_from_coordinator(
            &coordinator.lock(),
            BackgroundExtractMode::Balanced,
        ));
        assert!(recording.activated_windows.lock().iter().any(|window| !window.is_empty()));

        let mut handler =
            QueueEventHandler::new(playback, Arc::clone(&queue), Arc::clone(&play_queue))
                .with_media_preparer(preparer)
                .with_playback_coordinator(Arc::clone(&coordinator))
                .with_plan_changed({
                    let coordinator = Arc::clone(&coordinator);
                    let recording = Arc::clone(&recording);
                    Arc::new(move || {
                        let plan = media_plan_from_coordinator(
                            &coordinator.lock(),
                            BackgroundExtractMode::Balanced,
                        );
                        recording.apply_plan(plan);
                    })
                });

        handler.handle(event);

        assert_eq!(recording.activated_windows.lock().last().cloned(), Some(Vec::new()));
    }

    #[test]
    fn handle_stopped_clears_media_window_via_plan_publication() {
        assert_reset_event_clears_media_via_plan(QueueEvent::Stopped);
    }

    #[test]
    fn handle_cleared_clears_media_window_via_plan_publication() {
        assert_reset_event_clears_media_via_plan(QueueEvent::Cleared);
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
