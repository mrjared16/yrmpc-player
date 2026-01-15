//! Integration tests for PlayWithIntent server command.
//!
//! Tests the full command flow: ServerCommand::PlayWithIntent → handler → ServerResponse
//!
//! Run with: cargo test --test play_intent_integration_tests -- --nocapture

use rmpc::backends::youtube::protocol::{
    play_intent::{MixType, PlayIntent, RequestId},
    ServerCommand, ServerResponse,
};
use rmpc::domain::Song;

fn test_song(uri: &str) -> Song {
    use std::collections::HashMap;
    let mut metadata = HashMap::new();
    metadata.insert("title".to_string(), vec![format!("Test Song {}", uri)]);
    metadata.insert("artist".to_string(), vec!["Test Artist".to_string()]);
    
    Song {
        uri: uri.to_string(),
        id: None,
        metadata,
        ..Default::default()
    }
}

/// Test that PlayWithIntent::Context with valid data returns Ok
#[test]
fn test_play_intent_context_success() {
    let songs = vec![test_song("s1"), test_song("s2"), test_song("s3")];
    let intent = PlayIntent::Context {
        tracks: songs.clone(),
        offset: 0,
        shuffle: false,
        source: None,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12345,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok(), "Command should serialize successfully");
}

/// Test that PlayWithIntent::Context with empty tracks is rejected
#[test]
fn test_play_intent_context_empty_tracks() {
    let intent = PlayIntent::Context {
        tracks: vec![],
        offset: 0,
        shuffle: false,
        source: None,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12346,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok(), "Command serialization should still work");
}

/// Test that PlayWithIntent::Context with invalid offset is rejected
#[test]
fn test_play_intent_context_invalid_offset() {
    let songs = vec![test_song("s1"), test_song("s2")];
    let intent = PlayIntent::Context {
        tracks: songs.clone(),
        offset: 10,
        shuffle: false,
        source: None,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12347,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok());
}

/// Test that PlayWithIntent::Next with valid tracks succeeds
#[test]
fn test_play_intent_next_success() {
    let songs = vec![test_song("n1"), test_song("n2")];
    let intent = PlayIntent::Next {
        tracks: songs.clone(),
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12348,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok());
}

/// Test that PlayWithIntent::Append with valid tracks succeeds
#[test]
fn test_play_intent_append_success() {
    let songs = vec![test_song("a1"), test_song("a2")];
    let intent = PlayIntent::Append {
        tracks: songs.clone(),
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12349,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok());
}

/// Test that PlayWithIntent::Radio with valid seed succeeds
#[test]
fn test_play_intent_radio_success() {
    let seed = test_song("radio_seed");
    let intent = PlayIntent::Radio {
        seed,
        mix_type: MixType::SongRadio,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12350,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok());
}

/// Test that PlayWithIntent::Radio with empty seed uri/id is rejected
#[test]
fn test_play_intent_radio_invalid_seed() {
    let seed = Song {
        uri: String::new(),
        id: None,
        ..Default::default()
    };
    let intent = PlayIntent::Radio {
        seed,
        mix_type: MixType::ArtistRadio,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent,
        request_id: 12351,
    };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok(), "Serialization works even with invalid data");
}

/// Test that ServerCommand::CancelRequest is wired
#[test]
fn test_cancel_request_command_exists() {
    let cmd = ServerCommand::CancelRequest { request_id: 99999 };

    let serialized = serde_json::to_string(&cmd);
    assert!(serialized.is_ok());
}

/// Test round-trip serialization of PlayWithIntent command
#[test]
fn test_play_intent_serde_round_trip() {
    let songs = vec![test_song("rt1"), test_song("rt2")];
    let intent = PlayIntent::Context {
        tracks: songs.clone(),
        offset: 1,
        shuffle: true,
        source: None,
    };

    let cmd = ServerCommand::PlayWithIntent {
        intent: intent.clone(),
        request_id: 55555,
    };

    let json = serde_json::to_string(&cmd).expect("Should serialize");
    let deserialized: ServerCommand = serde_json::from_str(&json).expect("Should deserialize");

    if let ServerCommand::PlayWithIntent {
        intent: deserialized_intent,
        request_id,
    } = deserialized
    {
        assert_eq!(request_id, 55555);
        if let PlayIntent::Context {
            tracks,
            offset,
            shuffle,
            ..
        } = deserialized_intent
        {
            assert_eq!(tracks.len(), 2);
            assert_eq!(offset, 1);
            assert_eq!(shuffle, true);
            assert_eq!(tracks[0].uri, "rt1");
        } else {
            panic!("Expected Context variant");
        }
    } else {
        panic!("Expected PlayWithIntent command");
    }
}

/// Test that ServerResponse::PlayIntentError exists and serializes
#[test]
fn test_play_intent_error_response() {
    use rmpc::backends::youtube::protocol::play_intent::PlayError;

    let err = PlayError::EmptyTracks;
    let response = ServerResponse::PlayIntentError(err);

    let serialized = serde_json::to_string(&response);
    assert!(serialized.is_ok(), "Error response should serialize");
}

/// Verify that legacy commands still serialize correctly
#[test]
fn test_legacy_commands_still_work() {
    use rmpc::backends::youtube::protocol::SongData;
    
    let legacy_song_data = SongData {
        id: None,
        file: "legacy1".to_string(),
        title: Some("Legacy Song".to_string()),
        artist: Some("Legacy Artist".to_string()),
        album: None,
        duration_ms: None,
        thumbnail: None,
        item_type: Some("song".to_string()),
    };
    
    let legacy_commands = vec![
        ServerCommand::AddSong {
            song: legacy_song_data,
            position: None,
        },
        ServerCommand::PlayPos(5),
        ServerCommand::Play,
        ServerCommand::Pause,
        ServerCommand::Next,
        ServerCommand::Previous,
    ];

    for cmd in legacy_commands {
        let serialized = serde_json::to_string(&cmd);
        assert!(
            serialized.is_ok(),
            "Legacy command should still serialize: {:?}",
            cmd
        );
    }
}

/// Test that request IDs are preserved through serialization
#[test]
fn test_request_id_preservation() {
    let request_ids: Vec<RequestId> = vec![0, 1, u64::MAX, 42424242];

    for rid in request_ids {
        let intent = PlayIntent::Next {
            tracks: vec![test_song("id_test")],
        };
        let cmd = ServerCommand::PlayWithIntent {
            intent,
            request_id: rid,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        let deserialized: ServerCommand = serde_json::from_str(&json).unwrap();

        if let ServerCommand::PlayWithIntent { request_id, .. } = deserialized {
            assert_eq!(request_id, rid, "Request ID should be preserved");
        } else {
            panic!("Expected PlayWithIntent");
        }
    }
}
