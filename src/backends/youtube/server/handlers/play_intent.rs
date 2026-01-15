//! PlayIntent handler for declarative playback control.
//!
//! This handler processes PlayWithIntent commands, which express user intent
//! declaratively rather than imperatively.
//!
//! Uses the unified CacheExecutor for all cache work (preload + prepare).

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    protocol::{
        ServerResponse,
        play_intent::{PlayError, PlayIntent, PreloadTier, RequestId, derive_priorities},
    },
    services::{
        CacheExecutorHandle,
        PlaybackService,
        PlaybackStateTracker,
        QueueService,
    },
    server::orchestrator,
};

pub fn handle_play_with_intent(
    intent: PlayIntent,
    request_id: RequestId,
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    event_tx: &Sender<String>,
    cache_executor: &CacheExecutorHandle,
) -> ServerResponse {
    if let Err(e) = validate_intent(&intent) {
        log::warn!("[INTENT] validation_failed request_id={} error={:?}", request_id, e);
        return ServerResponse::PlayIntentError(e);
    }

    let priorities = derive_priorities(&intent);

    for (song, tier) in &priorities {
        let Some(track_id) = extract_video_id(&song.uri) else {
            continue;
        };

        cache_executor.preload(track_id, *tier, request_id);
    }

    match &intent {
        PlayIntent::Context { tracks, shuffle, offset, .. } => {
            log::info!(
                "[INTENT] context request_id={} count={} shuffle={} offset={}",
                request_id, tracks.len(), shuffle, offset
            );

            queue.clear();
            for song in tracks {
                queue.add(song.clone(), None);
            }

            if *shuffle {
                queue.set_shuffle_enabled(true);
            }

            let _ = event_tx.send("queue".to_string());
            
            return orchestrator::play_position(playback, queue, *offset, state_tracker);
        }

        PlayIntent::Next { tracks } => {
            log::info!("[INTENT] next request_id={} count={}", request_id, tracks.len());

            let insert_pos = queue.current_index().map(|i| i + 1);
            for (i, song) in tracks.iter().enumerate() {
                let pos = insert_pos.map(|p| (p + i) as u32);
                queue.add(song.clone(), pos);
            }

            let _ = event_tx.send("queue".to_string());
        }

        PlayIntent::Append { tracks } => {
            log::info!("[INTENT] append request_id={} count={}", request_id, tracks.len());

            for song in tracks {
                queue.add(song.clone(), None);
            }

            let _ = event_tx.send("queue".to_string());
        }

        PlayIntent::Radio { seed, mix_type } => {
            log::info!(
                "[INTENT] radio request_id={} seed={} mix_type={:?}",
                request_id, &seed.uri, mix_type
            );

            queue.clear();
            queue.add(seed.clone(), None);

            let _ = event_tx.send("queue".to_string());
            
            return orchestrator::play_position(playback, queue, 0, state_tracker);
        }
    }

    ServerResponse::Ok
}

/// Validate PlayIntent before processing
fn extract_video_id(uri: &str) -> Option<String> {
    if let Some(id) = uri.strip_prefix("youtube://") {
        return Some(id.to_string());
    }

    if !uri.is_empty() && !uri.contains("://") {
        return Some(uri.to_string());
    }

    None
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
    use super::*;
    use crate::domain::Song;

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
}
