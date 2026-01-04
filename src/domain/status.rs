use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Play,
    Pause,
    Stop,
}
impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Play => write!(f, "play"),
            State::Pause => write!(f, "pause"),
            State::Stop => write!(f, "stop"),
        }
    }
}
impl From<State> for crate::mpd::commands::State {
    fn from(val: State) -> Self {
        match val {
            State::Play => crate::mpd::commands::State::Play,
            State::Pause => crate::mpd::commands::State::Pause,
            State::Stop => crate::mpd::commands::State::Stop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnOffOneshot {
    On,
    Off,
    Oneshot,
}

impl Default for OnOffOneshot {
    fn default() -> Self {
        Self::Off
    }
}

impl OnOffOneshot {
    pub fn cycle(self) -> Self {
        match self {
            OnOffOneshot::On => OnOffOneshot::Off,
            OnOffOneshot::Off => OnOffOneshot::Oneshot,
            OnOffOneshot::Oneshot => OnOffOneshot::On,
        }
    }

    pub fn cycle_skip_oneshot(&self) -> Self {
        match self {
            OnOffOneshot::On => OnOffOneshot::Off,
            OnOffOneshot::Off => OnOffOneshot::On,
            OnOffOneshot::Oneshot => OnOffOneshot::On,
        }
    }
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

    /// Single mode
    pub single: OnOffOneshot,

    /// Consume mode
    pub consume: OnOffOneshot,

    /// Playlist version (if applicable)
    pub playlist: Option<u32>,

    /// Playlist length
    pub playlistlength: u32,

    /// Elapsed time in current song
    pub elapsed: Option<Duration>,

    /// Total duration of current song
    pub duration: Option<Duration>,

    /// Current song ID (MPD uses 'songid')
    pub songid: Option<u32>,

    /// Next song ID (if applicable)
    pub next_songid: Option<u32>,

    /// Current position in queue (if applicable)
    pub song_position: Option<u32>,

    /// Next position in queue (for shuffle mode visual indicator)
    pub next_song_position: Option<u32>,

    /// Bitrate in kbps (if available)
    pub bitrate: Option<u32>,

    /// Error message (if any)
    pub error: Option<String>,

    /// Updating DB job ID
    pub updating_db: Option<u32>,

    /// Crossfade duration in seconds
    pub xfade: Option<u32>,

    /// Partition name (MPD-specific)
    pub partition: String,

    /// Last loaded playlist name (MPD-specific)
    pub lastloadedplaylist: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: State::Stop,
            volume: 100,
            repeat: false,
            random: false,
            single: OnOffOneshot::Off,
            consume: OnOffOneshot::Off,
            playlist: None,
            playlistlength: 0,
            elapsed: None,
            duration: None,
            songid: None,
            next_songid: None,
            song_position: None,
            next_song_position: None,
            bitrate: None,
            error: None,
            updating_db: None,
            xfade: None,
            partition: String::from("default"),
            lastloadedplaylist: None,
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

impl From<crate::mpd::commands::OnOffOneshot> for OnOffOneshot {
    fn from(mpd_val: crate::mpd::commands::OnOffOneshot) -> Self {
        match mpd_val {
            crate::mpd::commands::OnOffOneshot::On => OnOffOneshot::On,
            crate::mpd::commands::OnOffOneshot::Off => OnOffOneshot::Off,
            crate::mpd::commands::OnOffOneshot::Oneshot => OnOffOneshot::Oneshot,
        }
    }
}

impl From<OnOffOneshot> for crate::mpd::commands::OnOffOneshot {
    fn from(val: OnOffOneshot) -> Self {
        match val {
            OnOffOneshot::On => crate::mpd::commands::OnOffOneshot::On,
            OnOffOneshot::Off => crate::mpd::commands::OnOffOneshot::Off,
            OnOffOneshot::Oneshot => crate::mpd::commands::OnOffOneshot::Oneshot,
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
            single: mpd_status.single.into(),
            consume: mpd_status.consume.into(),
            playlist: mpd_status.playlist,
            playlistlength: mpd_status.playlistlength,
            elapsed: Some(mpd_status.elapsed),
            duration: Some(mpd_status.duration),
            songid: mpd_status.songid,
            next_songid: mpd_status.nextsongid,
            song_position: mpd_status.song,
            next_song_position: mpd_status.nextsong,
            bitrate: mpd_status.bitrate,
            error: mpd_status.error,
            updating_db: mpd_status.updating_db,
            xfade: mpd_status.xfade,
            partition: mpd_status.partition,
            lastloadedplaylist: mpd_status.lastloadedplaylist,
        }
    }
}
