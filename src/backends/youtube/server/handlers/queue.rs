//! Queue management handlers.

use std::sync::Arc;

use crate::{
    backends::youtube::{
        protocol::{ServerResponse, SongData},
        server::queue_coordinator::QueueCoordinator,
        services::{PlaybackService, QueueService},
    },
    domain::Song,
    shared::play_queue::PlayQueue,
};

/// Handle Add command (URI only, legacy)
pub fn handle_add(
    queue_coordinator: &QueueCoordinator,
    uri: &str,
    position: Option<u32>,
) -> ServerResponse {
    queue_coordinator.add_uri(uri, position)
}

/// Handle AddSong command with full metadata
pub fn handle_add_song(
    queue_coordinator: &QueueCoordinator,
    song_data: SongData,
    position: Option<u32>,
) -> ServerResponse {
    queue_coordinator.add_song(song_data, position)
}

/// Handle DeleteId command
pub fn handle_delete_id(queue_coordinator: &QueueCoordinator, id: u32) -> ServerResponse {
    queue_coordinator.delete_id(id)
}

/// Handle Clear command
pub fn handle_clear(queue_coordinator: &QueueCoordinator) -> ServerResponse {
    queue_coordinator.clear()
}

/// Handle MoveId command
pub fn handle_move_id(queue_coordinator: &QueueCoordinator, from: u32, to: u32) -> ServerResponse {
    queue_coordinator.move_id(from, to)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        backends::youtube::{
            audio::AudioDeliveryPlanner,
            config::{AudioDeliveryMode, ExtractorType},
            media::{MediaPreparer, PreloadTier, PreparedMedia},
            server::{
                handlers::queue_events::QueueEventHandler,
                orchestrator::Orchestrator,
                queue_coordinator::QueueCoordinator,
                test_support::{MpvTestGuard, RecordingMediaPreparer, acquire_mpv_test_guard},
            },
            services::PlaybackStateTracker,
            url_resolver::UrlResolver,
        },
        shared::play_queue::PlayQueue,
    };

    struct StubMediaPreparer;

    #[async_trait]
    impl MediaPreparer for StubMediaPreparer {
        async fn prepare(&self, track_id: &str, _tier: PreloadTier) -> Result<PreparedMedia> {
            Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
        }

        fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}
    }

    fn test_song(uri: &str) -> SongData {
        SongData::from(Song { uri: uri.to_string(), ..Song::default() })
    }

    fn setup_queue_harness()
    -> (MpvTestGuard, TempDir, Arc<QueueCoordinator>, Arc<QueueService>, Arc<Mutex<PlayQueue>>)
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
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let state_tracker = Arc::new(PlaybackStateTracker::new());
        let media_preparer: Arc<dyn MediaPreparer> = Arc::new(StubMediaPreparer);
        let orchestrator = Orchestrator::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            state_tracker,
            media_preparer,
            crate::backends::youtube::config::BackgroundExtractMode::Balanced,
            2,
        );
        let queue_event_handler = Mutex::new(QueueEventHandler::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            Arc::clone(&play_queue),
        ));
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let queue_coordinator = Arc::new(QueueCoordinator::new(
            Arc::clone(&queue),
            Arc::clone(&playback),
            Arc::clone(&play_queue),
            queue_event_handler.into_inner(),
            Arc::new(orchestrator),
            event_tx,
        ));

        (mpv_guard, temp_dir, queue_coordinator, queue, play_queue)
    }

    #[test]
    fn handle_add_song_keeps_play_queue_order_aligned_with_positioned_insert() {
        let (_mpv_guard, _temp_dir, queue_coordinator, queue, play_queue) = setup_queue_harness();

        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-0"), None),
            ServerResponse::Ok
        ));
        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-2"), None),
            ServerResponse::Ok
        ));

        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-1"), Some(1)),
            ServerResponse::Ok
        ));

        let queue_titles: Vec<String> =
            (0..queue.len()).map(|idx| queue.get_by_index(idx).unwrap().uri).collect();
        let play_queue_titles: Vec<String> = {
            let play_queue = play_queue.lock();
            play_queue
                .get_play_order()
                .iter()
                .map(|id| play_queue.get_song(*id).unwrap().uri.clone())
                .collect()
        };

        assert_eq!(queue_titles, vec!["song-0", "song-1", "song-2"]);
        assert_eq!(play_queue_titles, queue_titles);
    }

    #[test]
    fn handle_add_song_outside_active_window_defers_background_work_to_coordinator_policy() {
        let _mpv_guard = acquire_mpv_test_guard();
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
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let state_tracker = Arc::new(PlaybackStateTracker::new());
        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();
        let orchestrator = Orchestrator::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            state_tracker,
            Arc::clone(&media_preparer),
            crate::backends::youtube::config::BackgroundExtractMode::Balanced,
            2,
        );
        let queue_event_handler = Mutex::new(
            QueueEventHandler::new(
                Arc::clone(&playback),
                Arc::clone(&queue),
                Arc::clone(&play_queue),
            )
            .with_media_preparer(media_preparer),
        );
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let queue_coordinator = QueueCoordinator::new(
            Arc::clone(&queue),
            Arc::clone(&playback),
            Arc::clone(&play_queue),
            queue_event_handler.into_inner(),
            Arc::new(orchestrator),
            event_tx,
        );

        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-0"), None),
            ServerResponse::Ok
        ));
        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-1"), None),
            ServerResponse::Ok
        ));
        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-2"), None),
            ServerResponse::Ok
        ));

        recording.warmed_batches.lock().clear();
        recording.warmed.lock().clear();

        queue.set_current(Some(0));

        assert!(matches!(
            handle_add_song(&queue_coordinator, test_song("song-999"), Some(3)),
            ServerResponse::Ok
        ));

        assert!(recording.warmed.lock().is_empty());
        assert!(recording.warmed_batches.lock().is_empty());
    }
}
