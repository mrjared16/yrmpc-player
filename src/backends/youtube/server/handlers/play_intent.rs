//! PlayIntent handler for declarative playback control.
//!
//! This handler processes PlayWithIntent commands, which express user intent
//! declaratively rather than imperatively.
//!
//! Uses the unified YouTubeMediaPreparer for all cache work (preload +
//! prepare).

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    media::MediaPreparer,
    protocol::{
        ServerResponse,
        play_intent::{PlayError, PlayIntent, RequestId},
    },
    server::{
        handlers::stable_track_id,
        orchestrator::{Orchestrator, PREFETCH_WINDOW_SIZE},
        queue_coordinator::QueueCoordinator,
    },
};
use crate::domain::Song;
use crate::shared::play_queue::{QueueCommand, QueueInsertPlacement};

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

    match &intent {
        PlayIntent::Replace(replace) => {
            let source = context_source_label(&intent);
            log::info!(
                "[INTENT] replace request_id={} source={} count={} shuffle={} start={:?}",
                request_id,
                source,
                replace.tracks.len(),
                matches!(
                    replace.order,
                    crate::backends::youtube::protocol::play_intent::QueueOrder::Shuffle
                ),
                replace.playback
            );
            if !matches!(
                replace.target,
                crate::backends::youtube::protocol::play_intent::QueueTarget::Main
            ) {
                return ServerResponse::PlayIntentError(PlayError::UnsupportedQueueTarget);
            }
            orchestrator.queue().clear();
            for song in &replace.tracks {
                orchestrator.queue().add(song.clone(), None);
            }

            queue_coordinator.apply(QueueCommand::Clear);
            queue_coordinator.apply(QueueCommand::Insert {
                songs: replace.tracks.clone(),
                placement: QueueInsertPlacement::End,
            });

            let shuffle = matches!(
                replace.order,
                crate::backends::youtube::protocol::play_intent::QueueOrder::Shuffle
            );
            orchestrator.queue().set_shuffle_enabled(shuffle);
            queue_coordinator.apply(QueueCommand::SetShuffle { enabled: shuffle });

            let _ = event_tx.send("queue".to_string());
            if let crate::backends::youtube::protocol::play_intent::ReplacePlayback::StartAtIndex(
                offset,
            ) = replace.playback
            {
                return orchestrator.play_position_sync(offset);
            }
        }

        PlayIntent::Insert(insert) => {
            log::info!("[INTENT] insert request_id={} count={}", request_id, insert.tracks.len());
            if !matches!(
                insert.target,
                crate::backends::youtube::protocol::play_intent::QueueTarget::Main
            ) {
                return ServerResponse::PlayIntentError(PlayError::UnsupportedQueueTarget);
            }
            let had_active_playback = orchestrator.queue().current_index().is_some();
            let base = orchestrator.queue().playback_base_index();
            let queue_len = orchestrator.queue().len();
            let insert_pos = match insert.placement {
                crate::backends::youtube::protocol::play_intent::InsertPlacement::End => queue_len,
                crate::backends::youtube::protocol::play_intent::InsertPlacement::AfterCurrent => {
                    orchestrator.queue().current_index().map(|i| i + 1).unwrap_or(queue_len)
                }
                crate::backends::youtube::protocol::play_intent::InsertPlacement::Absolute(n) => {
                    n.min(queue_len)
                }
            };

            for (i, song) in insert.tracks.iter().enumerate() {
                orchestrator.queue().add(song.clone(), Some((insert_pos + i) as u32));
            }

            queue_coordinator.apply(QueueCommand::Insert {
                songs: insert.tracks.clone(),
                placement: QueueInsertPlacement::Absolute(insert_pos),
            });

            if had_active_playback && insert_pos < base + PREFETCH_WINDOW_SIZE {
                if let Err(e) = orchestrator.reconcile_active_window_after_queue_mutation() {
                    log::warn!("Failed to reconcile active playback window after insert: {e}");
                }
            }

            let _ = event_tx.send("playlist".to_string());
            if let crate::backends::youtube::protocol::play_intent::InsertPlayback::StartInserted { index } = insert.playback {
                return orchestrator.play_position_sync(insert_pos + index);
            }
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
            queue_coordinator.apply(QueueCommand::Insert {
                songs: vec![seed.clone()],
                placement: QueueInsertPlacement::End,
            });

            let _ = event_tx.send("queue".to_string());

            return orchestrator.play_position_sync(0);
        }
    }

    ServerResponse::Ok
}

fn context_source_label(intent: &PlayIntent) -> &'static str {
    match intent {
        PlayIntent::Replace(replace) => match &replace.source {
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::Album {
                ..
            }) => "album",
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::Playlist {
                ..
            }) => "playlist",
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::Artist {
                ..
            }) => "artist",
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::Search {
                ..
            }) => "search",
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::History) => {
                "history"
            }
            Some(crate::backends::youtube::protocol::play_intent::ContextSource::Queue) => "queue",
            None => "unknown",
        },
        _ => "n/a",
    }
}

/// Validate PlayIntent before processing
fn validate_intent(intent: &PlayIntent) -> Result<(), PlayError> {
    match intent {
        PlayIntent::Replace(replace) => {
            if replace.tracks.is_empty() {
                return Err(PlayError::EmptyTracks);
            }
            if let crate::backends::youtube::protocol::play_intent::ReplacePlayback::StartAtIndex(
                offset,
            ) = replace.playback
            {
                if offset >= replace.tracks.len() {
                    return Err(PlayError::InvalidOffset { offset, len: replace.tracks.len() });
                }
            }
        }
        PlayIntent::Insert(insert) => {
            if insert.tracks.is_empty() {
                return Err(PlayError::EmptyTracks);
            }
            if let crate::backends::youtube::protocol::play_intent::InsertPlayback::StartInserted {
                index,
            } = insert.playback
                && index >= insert.tracks.len()
            {
                return Err(PlayError::InvalidOffset { offset: index, len: insert.tracks.len() });
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
        shared::play_queue::{PlayQueue, QueueInsertPlacement},
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
        let orchestrator = Arc::new(Orchestrator::new(
            playback,
            queue,
            state_tracker,
            media_preparer,
            crate::backends::youtube::config::BackgroundExtractMode::Balanced,
            2,
        ));
        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));
        let queue_event_handler = Mutex::new(
            QueueEventHandler::new(
                Arc::clone(orchestrator.playback()),
                Arc::clone(orchestrator.queue()),
                Arc::clone(&play_queue),
            )
            .with_media_preparer(recording.clone())
            .with_playback_coordinator(Arc::clone(orchestrator.coordinator()))
            .with_plan_changed({
                let orchestrator = Arc::clone(&orchestrator);
                Arc::new(move || orchestrator.publish_prefix_plan_changed())
            }),
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
        let intent = PlayIntent::replace_and_play(vec![], 0, false, None);
        assert!(matches!(validate_intent(&intent), Err(PlayError::EmptyTracks)));
    }

    #[test]
    fn test_validate_context_invalid_offset() {
        let intent =
            PlayIntent::replace_and_play(vec![test_song("s1"), test_song("s2")], 5, false, None);
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::InvalidOffset { offset: 5, len: 2 })
        ));
    }

    #[test]
    fn test_validate_context_valid() {
        let intent =
            PlayIntent::replace_and_play(vec![test_song("s1"), test_song("s2")], 1, false, None);
        assert!(validate_intent(&intent).is_ok());
    }

    #[test]
    fn test_validate_next_empty() {
        let intent = PlayIntent::add_next(vec![]);
        assert!(matches!(validate_intent(&intent), Err(PlayError::EmptyTracks)));
    }

    #[test]
    fn test_validate_append_empty() {
        let intent = PlayIntent::add_last(vec![]);
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
    fn append_intent_does_not_schedule_background_warm_side_effects() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, recording, play_queue) =
            setup_orchestrator();
        orchestrator.queue().add(test_song("youtube://existing"), None);
        orchestrator.queue().set_current(Some(0));

        let intent = PlayIntent::add_last(vec![
            test_song("youtube://new-track-a"),
            test_song("youtube://new-track-b"),
        ]);
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();

        let response =
            handle_play_with_intent(intent, 42, &orchestrator, &queue_coordinator, &event_tx);

        assert!(matches!(response, ServerResponse::Ok));
        assert!(recording.warmed.lock().is_empty());
        assert!(recording.warmed_batches.lock().is_empty());
    }

    #[test]
    fn context_intent_keeps_play_queue_in_sync() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, _recording, play_queue) =
            setup_orchestrator();
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let tracks = vec![test_song("youtube://ctx-a"), test_song("youtube://ctx-b")];

        let response = handle_play_with_intent(
            PlayIntent::replace_and_play(tracks.clone(), 0, false, None),
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
    fn context_intent_defers_future_warm_until_playback_started() {
        let (_mpv_guard, _temp_dir, orchestrator, queue_coordinator, recording, _play_queue) =
            setup_orchestrator();
        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let tracks = vec![
            test_song("youtube://ctx-a"),
            test_song("youtube://ctx-b"),
            test_song("youtube://ctx-c"),
        ];

        let response = handle_play_with_intent(
            PlayIntent::replace_and_play(tracks, 0, false, None),
            17,
            &orchestrator,
            &queue_coordinator,
            &event_tx,
        );

        assert!(matches!(response, ServerResponse::Ok));
        let prepared = recording.prepared.lock().clone();
        assert_eq!(
            prepared.first(),
            Some(&("ctx-a".to_string(), crate::backends::youtube::media::PreloadTier::Immediate))
        );
        assert!(recording.warmed.lock().is_empty());
        assert!(recording.warmed_batches.lock().is_empty());

        orchestrator.handle_playback_started_for_current_track();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let prepared = recording.prepared.lock().clone();
            if prepared
                == vec![
                    ("ctx-a".to_string(), crate::backends::youtube::media::PreloadTier::Immediate),
                    ("ctx-b".to_string(), crate::backends::youtube::media::PreloadTier::Background),
                    ("ctx-c".to_string(), crate::backends::youtube::media::PreloadTier::Background),
                ]
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "unexpected prepared list: {prepared:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(recording.warmed.lock().is_empty());
        assert!(recording.warmed_batches.lock().is_empty());
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
            pq.apply(QueueCommand::Insert {
                songs: vec![test_song("youtube://existing-0")],
                placement: QueueInsertPlacement::End,
            });
            pq.apply(QueueCommand::Insert {
                songs: vec![test_song("youtube://existing-1")],
                placement: QueueInsertPlacement::End,
            });
        }

        let response = handle_play_with_intent(
            PlayIntent::add_next(vec![
                test_song("youtube://next-a"),
                test_song("youtube://next-b"),
            ]),
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
            pq.apply(QueueCommand::Insert {
                songs: vec![test_song("youtube://old")],
                placement: QueueInsertPlacement::End,
            });
            pq.apply(QueueCommand::Insert {
                songs: vec![test_song("youtube://old-2")],
                placement: QueueInsertPlacement::End,
            });
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
