//! Queue management handlers.

use std::sync::Arc;

use crate::{
    backends::youtube::{
        protocol::ServerResponse,
        server::queue_coordinator::QueueCoordinator,
        services::{PlaybackService, QueueService},
    },
    domain::Song,
    shared::play_queue::PlayQueue,
};

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

    fn test_song(uri: &str) -> Song {
        Song { uri: uri.to_string(), ..Song::default() }
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
}
