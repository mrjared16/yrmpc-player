//! PlayIntent handler for declarative playback control.
//!
//! This handler processes PlayWithIntent commands, which express user intent
//! declaratively rather than imperatively. The handler:
//! 1. Validates the intent (empty tracks, invalid offset)
//! 2. Derives preload priorities (logged only in Phase 1b, scheduled in Phase 1c)
//! 3. Mutates queue (stubbed - actual wiring in task 1.4)
//! 4. Initiates playback for Context/Radio intents

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    protocol::{
        play_intent::{derive_priorities, PlayError, PlayIntent, RequestId},
        ServerResponse,
    },
    services::{PlaybackService, PlaybackStateTracker, QueueService},
};

/// Handle PlayWithIntent command
///
/// This is the daemon-side handler for the new PlayIntent architecture.
/// It validates the intent, derives preload priorities, and coordinates
/// queue mutation and playback.
///
/// # Phase 1b (Current Implementation)
/// - Validation: Reject empty tracks, invalid offsets
/// - Priority derivation: Call derive_priorities() and log results
/// - Queue mutation: Log what would happen (stub)
/// - Playback: Log play_pos() calls (stub)
///
/// # Future Phases
/// - Phase 1c: Wire up actual preload scheduler
/// - Phase 1.4: Wire up queue mutation (replace/insert/append)
pub fn handle_play_with_intent(
    intent: PlayIntent,
    request_id: RequestId,
    _playback: &Arc<PlaybackService>,
    _queue: &Arc<QueueService>,
    _state_tracker: &Arc<PlaybackStateTracker>,
    _event_tx: &Sender<String>,
) -> ServerResponse {
    if let Err(e) = validate_intent(&intent) {
        log::warn!("PlayWithIntent validation failed: request_id={}, error={:?}", request_id, e);
        return ServerResponse::PlayIntentError(e);
    }

    let priorities = derive_priorities(&intent);
    
    for (song, tier) in &priorities {
        log::debug!(
            "Preload priority derived: request_id={}, song_uri={}, tier={:?}",
            request_id,
            &song.uri,
            tier
        );
    }
    
    log::info!(
        "PlayWithIntent received: request_id={}, intent_type={:?}, track_count={}",
        request_id,
        std::mem::discriminant(&intent),
        priorities.len()
    );

    match &intent {
        PlayIntent::Context { tracks, shuffle, offset, .. } => {
            log::info!(
                "Would replace queue with tracks: request_id={}, count={}, shuffle={}, offset={}",
                request_id,
                tracks.len(),
                shuffle,
                offset
            );
        }
        PlayIntent::Next { tracks } => {
            log::info!(
                "Would insert tracks after current: request_id={}, count={}",
                request_id,
                tracks.len()
            );
        }
        PlayIntent::Append { tracks } => {
            log::info!(
                "Would append tracks to queue: request_id={}, count={}",
                request_id,
                tracks.len()
            );
        }
        PlayIntent::Radio { seed, mix_type } => {
            log::info!(
                "Would start radio from seed: request_id={}, seed_uri={}, mix_type={:?}",
                request_id,
                &seed.uri,
                mix_type
            );
        }
    }

    match &intent {
        PlayIntent::Context { offset, .. } => {
            log::info!(
                "Would call player.play_pos({}): request_id={}",
                offset,
                request_id
            );
        }
        PlayIntent::Radio { .. } => {
            log::info!("Would call player.play_pos(0): request_id={}", request_id);
        }
        PlayIntent::Next { .. } | PlayIntent::Append { .. } => {
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
                return Err(PlayError::InvalidOffset {
                    offset: *offset,
                    len: tracks.len(),
                });
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
        Song {
            uri: uri.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_context_empty_tracks() {
        let intent = PlayIntent::Context {
            tracks: vec![],
            offset: 0,
            shuffle: false,
            source: None,
        };
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::EmptyTracks)
        ));
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
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::EmptyTracks)
        ));
    }

    #[test]
    fn test_validate_append_empty() {
        let intent = PlayIntent::Append { tracks: vec![] };
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::EmptyTracks)
        ));
    }

    #[test]
    fn test_validate_radio_invalid_seed() {
        let seed = Song {
            uri: String::new(),
            id: None,
            ..Default::default()
        };
        let intent = PlayIntent::Radio {
            seed,
            mix_type: crate::backends::youtube::protocol::play_intent::MixType::SongRadio,
        };
        assert!(matches!(
            validate_intent(&intent),
            Err(PlayError::RadioSeedInvalid)
        ));
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
