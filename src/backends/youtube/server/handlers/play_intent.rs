//! PlayIntent handler for declarative playback control.
//!
//! This handler processes PlayWithIntent commands, which express user intent
//! declaratively rather than imperatively.
//!
//! Uses the unified YouTubeMediaPreparer for all cache work (preload +
//! prepare).

use std::sync::Arc;

use crossbeam::channel::Sender;

use super::extract_video_id;
use crate::backends::youtube::{
    media::{MediaPreparer, PreloadTier},
    protocol::{
        ServerResponse,
        play_intent::{PlayError, PlayIntent, RequestId, derive_priorities},
    },
    server::{
        orchestrator::{Orchestrator, PREFETCH_WINDOW_SIZE},
        queue_coordinator::QueueCoordinator,
    },
};
use crate::shared::play_queue::QueueCommand;

pub fn handle_play_with_intent(
    intent: PlayIntent,
    request_id: RequestId,
    orchestrator: &Orchestrator,
    queue_coordinator: &QueueCoordinator,
    event_tx: &Sender<String>,
) -> ServerResponse {
    if let Err(e) = validate_intent(&intent) {
        log::warn!("[INTENT] validation_failed request_id={} error={:?}", request_id, e);
        return ServerResponse::PlayIntentError(e);
    }

    let priorities = derive_priorities(&intent);
    if !matches!(&intent, PlayIntent::Append { .. }) {
        for (song, tier) in &priorities {
            let Some(track_id) = extract_video_id(&song.uri) else {
                continue;
            };

            orchestrator.media_preparer().prefetch(&track_id, *tier);
        }
    }

    match &intent {
        PlayIntent::Context { tracks, shuffle, offset, .. } => {
            log::info!(
                "[INTENT] context request_id={} count={} shuffle={} offset={}",
                request_id,
                tracks.len(),
                shuffle,
                offset
            );

            orchestrator.queue().clear();
            for song in tracks {
                orchestrator.queue().add(song.clone(), None);
            }

            queue_coordinator.apply(QueueCommand::Clear);
            queue_coordinator.apply(QueueCommand::AddBatch { songs: tracks.clone() });

            orchestrator.queue().set_shuffle_enabled(*shuffle);
            queue_coordinator.apply(QueueCommand::SetShuffle { enabled: *shuffle });

            let _ = event_tx.send("queue".to_string());

            return orchestrator.play_position_sync(*offset);
        }

        PlayIntent::Next { tracks } => {
            log::info!("[INTENT] next request_id={} count={}", request_id, tracks.len());

            let insert_pos = orchestrator.queue().current_index().map(|i| i + 1);
            for (i, song) in tracks.iter().enumerate() {
                let pos = insert_pos.map(|p| (p + i) as u32);
                orchestrator.queue().add(song.clone(), pos);
                let play_queue_position =
                    pos.unwrap_or(orchestrator.queue().len().saturating_sub(1) as u32);
                queue_coordinator.apply(QueueCommand::AddAt {
                    song: song.clone(),
                    position: play_queue_position as usize,
                });
            }

            let _ = event_tx.send("queue".to_string());
        }

        PlayIntent::Append { tracks } => {
            log::info!("[INTENT] append request_id={} count={}", request_id, tracks.len());
            let had_active_playback = orchestrator.queue().current_index().is_some();
            let base = orchestrator.queue().playback_base_index();
            let insert_pos = orchestrator.queue().len();

            for song in tracks {
                orchestrator.queue().add(song.clone(), None);
            }

            queue_coordinator.apply(QueueCommand::AddBatch { songs: tracks.clone() });

            if had_active_playback && insert_pos < base + PREFETCH_WINDOW_SIZE {
                if let Err(e) = orchestrator.reconcile_active_window_after_queue_mutation() {
                    log::warn!("Failed to reconcile active playback window after append: {e}");
                }
            }

            let _ = event_tx.send("playlist".to_string());
        }

        PlayIntent::Radio { seed, mix_type } => {
            log::info!(
                "[INTENT] radio request_id={} seed={} mix_type={:?}",
                request_id,
                &seed.uri,
                mix_type
            );

            orchestrator.queue().clear();
            orchestrator.queue().add(seed.clone(), None);

            queue_coordinator.apply(QueueCommand::Clear);
            queue_coordinator.apply(QueueCommand::Add { song: seed.clone() });

            let _ = event_tx.send("queue".to_string());

            return orchestrator.play_position_sync(0);
        }
    }

    ServerResponse::Ok
}

/// Validate PlayIntent before processing
fn validate_intent(intent: &PlayIntent) -> Result<(), PlayError> {
    match intent {
        PlayIntent::Context { tracks, offset, .. } => {
            if tracks.is_empty() {
                return Err(PlayError::EmptyTracks);
            }
            if *offset >= tracks.len() {
                return Err(PlayError::InvalidOffset { offset: *offset, len: tracks.len() });
            }
        }
        PlayIntent::Next { tracks } | PlayIntent::Append { tracks } => {
            if tracks.is_empty() {
                return Err(PlayError::EmptyTracks);
            }
        }
        PlayIntent::Radio { seed, .. } => {
            // Validate that seed has required fields
            if seed.id.is_none() && seed.uri.is_empty() {
                return Err(PlayError::RadioSeedInvalid);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    use crate::{
        backends::youtube::{
            audio::AudioDeliveryPlanner,
            config::{AudioDeliveryMode, ExtractorType},
            media::MediaPreparer,
            server::handlers::queue_events::QueueEventHandler,
            server::queue_coordinator::QueueCoordinator,
            server::test_support::{MpvTestGuard, RecordingMediaPreparer, acquire_mpv_test_guard},
            services::{PlaybackService, PlaybackStateTracker, QueueService},
            url_resolver::UrlResolver,
        },
        domain::Song,
        shared::play_queue::PlayQueue,
    };

    fn setup_orchestrator() -> (
        MpvTestGuard,
        TempDir,
        Arc<Orchestrator>,
        Arc<QueueCoordinator>,
        Arc<RecordingMediaPreparer>,
        Arc<Mutex<PlayQueue>>,
    ) {
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
        let state_tracker = Arc::new(PlaybackStateTracker::new());
        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();
        let orchestrator =
            Arc::new(Orchestrator::new(playback, queue, state_tracker, media_preparer));
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let queue_event_handler = Mutex::new(
            QueueEventHandler::new(
                Arc::clone(orchestrator.playback()),
                Arc::clone(orchestrator.queue()),
                Arc::clone(&play_queue),
            )
            .with_media_preparer(recording.clone()),
        );
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let queue_coordinator = Arc::new(QueueCoordinator::new(
            Arc::clone(orchestrator.queue()),
            Arc::clone(orchestrator.playback()),
            Arc::clone(&play_queue),
            queue_event_handler.into_inner(),
            Arc::clone(&orchestrator),
            event_tx,
        ));

        (mpv_guard, temp_dir, orchestrator, queue_coordinator, recording, play_queue)
    }

    fn test_song(uri: &str) -> Song {
        Song { uri: uri.to_string(), ..Default::default() }
    }

    #[test]
    fn test_validate_context_empty_tracks() {
        let intent =
            PlayIntent::Context { tracks: vec![], offset: 0, shuffle: false, source: None };
        assert!(matches!(validate_intent(&intent), Err(PlayError::EmptyTracks)));
    }

    #[test]
    fn test_validate_context_invalid_offset() {
        let intent = PlayIntent::Context {
            tracks: vec![test_song("s1"), test_song("s2")],
            offset: 5,
            shuffle: false,
            source: None,
        };
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::InvalidOffset { offset: 5, len: 2 })
        ));
    }

    #[test]
    fn test_validate_context_valid() {
        let intent = PlayIntent::Context {
            tracks: vec![test_song("s1"), test_song("s2")],
            offset: 1,
            shuffle: false,
            source: None,
        };
        assert!(validate_intent(&intent).is_ok());
    }

    #[test]
    fn test_validate_next_empty() {
        let intent = PlayIntent::Next { tracks: vec![] };
        assert!(matches!(validate_intent(&intent), Err(PlayError::EmptyTracks)));
    }

    #[test]
    fn test_validate_append_empty() {
        let intent = PlayIntent::Append { tracks: vec![] };
        assert!(matches!(validate_intent(&intent), Err(PlayError::EmptyTracks)));
    }

    #[test]
    fn test_validate_radio_invalid_seed() {
        let seed = Song { uri: String::new(), id: None, ..Default::default() };
        let intent = PlayIntent::Radio {
            seed,
            mix_type: crate::backends::youtube::protocol::play_intent::MixType::SongRadio,
        };
        assert!(matches!(validate_intent(&intent), Err(PlayError::RadioSeedInvalid)));
    }

    #[test]
    fn test_validate_radio_valid() {
        let seed = test_song("seed123");
        let intent = PlayIntent::Radio {
            seed,
            mix_type: crate::backends::youtube::protocol::play_intent::MixType::SongRadio,
        };
        assert!(validate_intent(&intent).is_ok());
    }

    #[test]
    fn append_intent_batches_warm_side_effects_when_playback_is_active() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, recording, play_queue) =
            setup_orchestrator();
        orchestrator.queue().add(test_song("youtube://existing"), None);
        orchestrator.queue().set_current(Some(0));

        let intent = PlayIntent::Append {
            tracks: vec![test_song("youtube://new-track-a"), test_song("youtube://new-track-b")],
        };
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();

        let response =
            handle_play_with_intent(intent, 42, &orchestrator, &queue_coordinator, &event_tx);

        assert!(matches!(response, ServerResponse::Ok));
        assert!(recording.warmed.lock().is_empty());
        assert_eq!(
            recording.warmed_batches.lock().clone(),
            vec![vec!["new-track-a".to_string(), "new-track-b".to_string()]]
        );
    }

    #[test]
    fn context_intent_keeps_play_queue_in_sync() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, _recording, play_queue) =
            setup_orchestrator();
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let tracks = vec![test_song("youtube://ctx-a"), test_song("youtube://ctx-b")];

        let response = handle_play_with_intent(
            PlayIntent::Context { tracks: tracks.clone(), offset: 0, shuffle: false, source: None },
            7,
            &orchestrator,
            &queue_coordinator,
            &event_tx,
        );

        assert!(matches!(response, ServerResponse::Ok));
        assert_eq!(orchestrator.queue().len(), 2);

        let play_queue = play_queue.lock();
        assert_eq!(play_queue.len(), 2);
        let ordered_uris: Vec<String> = play_queue
            .get_play_order()
            .iter()
            .filter_map(|id| play_queue.get_song(*id))
            .map(|song| song.uri.clone())
            .collect();
        assert_eq!(
            ordered_uris,
            vec!["youtube://ctx-a".to_string(), "youtube://ctx-b".to_string()]
        );
    }

    #[test]
    fn next_intent_inserts_into_play_queue_after_current() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, _recording, play_queue) =
            setup_orchestrator();
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();

        orchestrator.queue().add(test_song("youtube://existing-0"), None);
        orchestrator.queue().add(test_song("youtube://existing-1"), None);
        orchestrator.queue().set_current(Some(0));
        {
            let mut pq = play_queue.lock();
            pq.apply(QueueCommand::Add { song: test_song("youtube://existing-0") });
            pq.apply(QueueCommand::Add { song: test_song("youtube://existing-1") });
        }

        let response = handle_play_with_intent(
            PlayIntent::Next {
                tracks: vec![test_song("youtube://next-a"), test_song("youtube://next-b")],
            },
            8,
            &orchestrator,
            &queue_coordinator,
            &event_tx,
        );

        assert!(matches!(response, ServerResponse::Ok));

        let play_queue = play_queue.lock();
        let ordered_uris: Vec<String> = play_queue
            .get_play_order()
            .iter()
            .filter_map(|id| play_queue.get_song(*id))
            .map(|song| song.uri.clone())
            .collect();
        assert_eq!(
            ordered_uris,
            vec![
                "youtube://existing-0".to_string(),
                "youtube://next-a".to_string(),
                "youtube://next-b".to_string(),
                "youtube://existing-1".to_string(),
            ]
        );
    }

    #[test]
    fn radio_intent_resets_play_queue_to_seed() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, _recording, play_queue) =
            setup_orchestrator();
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();

        orchestrator.queue().add(test_song("youtube://old"), None);
        {
            let mut pq = play_queue.lock();
            pq.apply(QueueCommand::Add { song: test_song("youtube://old") });
            pq.apply(QueueCommand::Add { song: test_song("youtube://old-2") });
        }

        let response = handle_play_with_intent(
            PlayIntent::Radio {
                seed: test_song("youtube://seed"),
                mix_type: crate::backends::youtube::protocol::play_intent::MixType::SongRadio,
            },
            9,
            &orchestrator,
            &queue_coordinator,
            &event_tx,
        );

        assert!(matches!(response, ServerResponse::Ok));
        assert_eq!(orchestrator.queue().len(), 1);

        let play_queue = play_queue.lock();
        assert_eq!(play_queue.len(), 1);
        let only_id = play_queue.get_play_order()[0];
        assert_eq!(play_queue.get_song(only_id).map(|s| s.uri.as_str()), Some("youtube://seed"));
    }
}
