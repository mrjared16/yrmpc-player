//! IPC protocol for YouTube backend server-client communication.
//! This allows testing client and server independently.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::domain::{PlaybackState, Song, Status};

/// Commands sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerCommand {
    // Playback control
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    SeekAbsolute(f64),
    SeekRelative(f64),
    PlayPos(usize),
    PlayId(u32),

    // Queue management
    Add { uri: String, position: Option<u32> },
    DeleteId(u32),
    Clear,
    MoveId { from: u32, to: u32 },

    // Volume
    GetVolume,
    SetVolume(u8),
    AdjustVolume(i8),

    // Status/info
    GetStatus,
    GetCurrentSong,
    GetPlaylist,

    // Search/browse
    Search { query: String },
    Browse { path: String },

    // Library
    GetLibrary { category: String },

    // Lifecycle
    Ping,
    Shutdown,
}

/// Responses from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerResponse {
    Ok,
    Error(String),
    Status(StatusData),
    Song(Option<SongData>),
    Playlist(Vec<SongData>),
    SearchResults(Vec<SongData>),
    BrowseResults(Vec<BrowseEntry>),
    Library(Vec<BrowseEntry>),
    Volume(u8),
    Pong,
}

/// Serializable status data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusData {
    pub state: String, // "play", "pause", "stop"
    pub volume: u8,
    pub elapsed_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub playlist_length: u32,
    pub current_pos: Option<u32>,
    pub current_id: Option<u32>,
}

impl From<Status> for StatusData {
    fn from(s: Status) -> Self {
        Self {
            state: match s.state {
                PlaybackState::Play => "play".into(),
                PlaybackState::Pause => "pause".into(),
                PlaybackState::Stop => "stop".into(),
            },
            volume: s.volume,
            elapsed_ms: s.elapsed.map(|d| d.as_millis() as u64),
            duration_ms: s.duration.map(|d| d.as_millis() as u64),
            playlist_length: s.playlistlength,
            current_pos: s.song_position,
            current_id: s.songid,
        }
    }
}

impl StatusData {
    pub fn to_status(&self) -> Status {
        Status {
            state: match self.state.as_str() {
                "play" => PlaybackState::Play,
                "pause" => PlaybackState::Pause,
                _ => PlaybackState::Stop,
            },
            volume: self.volume,
            elapsed: self.elapsed_ms.map(Duration::from_millis),
            duration: self.duration_ms.map(Duration::from_millis),
            playlistlength: self.playlist_length,
            song_position: self.current_pos,
            songid: self.current_id,
            ..Default::default()
        }
    }
}

/// Serializable song data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongData {
    pub id: Option<u32>,
    pub file: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnail: Option<String>,
    pub item_type: Option<String>, // "song", "video", "album", "artist", "playlist", "header"
}

impl From<Song> for SongData {
    fn from(s: Song) -> Self {
        Self {
            id: s.id,
            file: s.file,
            title: s.metadata.get("title").and_then(|v| v.first()).cloned(),
            artist: s.metadata.get("artist").and_then(|v| v.first()).cloned(),
            album: s.metadata.get("album").and_then(|v| v.first()).cloned(),
            duration_ms: s.duration.map(|d| d.as_millis() as u64),
            thumbnail: s.metadata.get("thumbnail").and_then(|v| v.first()).cloned(),
            item_type: s.metadata.get("type").and_then(|v| v.first()).cloned(),
        }
    }
}

impl SongData {
    pub fn to_song(&self) -> Song {
        let mut metadata = std::collections::HashMap::new();
        if let Some(ref t) = self.title {
            metadata.insert("title".into(), vec![t.clone()]);
        }
        if let Some(ref a) = self.artist {
            metadata.insert("artist".into(), vec![a.clone()]);
        }
        if let Some(ref a) = self.album {
            metadata.insert("album".into(), vec![a.clone()]);
        }
        if let Some(ref t) = self.thumbnail {
            metadata.insert("thumbnail".into(), vec![t.clone()]);
        }
        if let Some(ref t) = self.item_type {
            metadata.insert("type".into(), vec![t.clone()]);
        }
        Song {
            id: self.id,
            file: self.file.clone(),
            duration: self.duration_ms.map(Duration::from_millis),
            metadata,
            ..Default::default()
        }
    }
}

/// Browse entry (directory or file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrowseEntry {
    Dir { name: String, path: String },
    File(SongData),
}

/// Frame-based protocol for reading/writing messages over socket
pub mod framing {
    use std::io::{Read, Write};

    use anyhow::{Context, Result};
    use serde::{Deserialize, Serialize};

    /// Write a message with length prefix
    pub fn write_message<W: Write, T: Serialize>(writer: &mut W, msg: &T) -> Result<()> {
        let json = serde_json::to_vec(msg)?;
        let len = json.len() as u32;
        writer.write_all(&len.to_le_bytes())?;
        writer.write_all(&json)?;
        writer.flush()?;
        Ok(())
    }

    /// Read a message with length prefix
    pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> Result<T> {
        let mut len_bytes = [0u8; 4];
        reader.read_exact(&mut len_bytes).context("Failed to read message length")?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        if len > 10 * 1024 * 1024 {
            anyhow::bail!("Message too large: {} bytes", len);
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).context("Failed to read message body")?;
        serde_json::from_slice(&buf).context("Failed to parse message")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_serialization() {
        let cmd = ServerCommand::Search { query: "test".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: ServerCommand = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerCommand::Search { query } => assert_eq!(query, "test"),
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_response_serialization() {
        let resp = ServerResponse::Status(StatusData {
            state: "play".into(),
            volume: 50,
            elapsed_ms: Some(1000),
            duration_ms: Some(180000),
            playlist_length: 5,
            current_pos: Some(0),
            current_id: Some(1),
        });
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ServerResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerResponse::Status(s) => {
                assert_eq!(s.state, "play");
                assert_eq!(s.volume, 50);
            }
            _ => panic!("Wrong response type"),
        }
    }

    #[test]
    fn test_framing_roundtrip() {
        use std::io::Cursor;

        let cmd = ServerCommand::PlayPos(5);
        let mut buf = Vec::new();
        framing::write_message(&mut buf, &cmd).unwrap();

        let mut cursor = Cursor::new(buf);
        let parsed: ServerCommand = framing::read_message(&mut cursor).unwrap();
        match parsed {
            ServerCommand::PlayPos(pos) => assert_eq!(pos, 5),
            _ => panic!("Wrong command type"),
        }
    }
}
