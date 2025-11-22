use std::time::Duration;
use serde::{Deserialize, Serialize};

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Play,
    Pause,
    Stop,
}

/// Domain model for player status
/// Backend-agnostic - can be populated from MPD, YouTube Music, Spotify, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// Current playback state
    pub state: State,
    
    /// Volume (0-100)
    pub volume: u8,
    
    /// Repeat mode enabled
    pub repeat: bool,
    
    /// Random/shuffle mode enabled
    pub random: bool,
    
    /// Elapsed time in current song
    pub elapsed: Option<Duration>,
    
    /// Total duration of current song
    pub duration: Option<Duration>,
    
    /// Current song ID (if applicable)
    pub song_id: Option<u32>,
    
    /// Next song ID (if applicable)
    pub next_song_id: Option<u32>,
    
    /// Current position in queue (if applicable)
    pub song_position: Option<u32>,
    
    /// Bitrate in kbps (if available)
    pub bitrate: Option<u32>,
    
    /// Error message (if any)
    pub error: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: State::Stop,
            volume: 100,
            repeat: false,
            random: false,
            elapsed: None,
            duration: None,
            song_id: None,
            next_song_id: None,
            song_position: None,
            bitrate: None,
            error: None,
        }
    }
}

// Conversion from MPD State to domain State
impl From<crate::mpd::commands::State> for State {
    fn from(mpd_state: crate::mpd::commands::State) -> Self {
        match mpd_state {
            crate::mpd::commands::State::Play => State::Play,
            crate::mpd::commands::State::Pause => State::Pause,
            crate::mpd::commands::State::Stop => State::Stop,
        }
    }
}

// Conversion from MPD Status to domain Status
impl From<crate::mpd::commands::status::Status> for Status {
    fn from(mpd_status: crate::mpd::commands::status::Status) -> Self {
        Self {
            state: mpd_status.state.into(),
            volume: mpd_status.volume.0 as u8,
            repeat: mpd_status.repeat,
            random: mpd_status.random,
            elapsed: Some(mpd_status.elapsed),
            duration: Some(mpd_status.duration),
            song_id: mpd_status.songid,
            next_song_id: mpd_status.nextsongid,
            song_position: mpd_status.song,
            bitrate: mpd_status.bitrate,
            error: mpd_status.error,
        }
    }
}
