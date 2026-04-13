#![allow(clippy::missing_errors_doc)]

//! Playback control traits and types.
//!
//! Controls the playback state: play, pause, stop, seek, volume.

use std::time::Duration;

use anyhow::Result;

/// Playback state
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum State {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Repeat mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

/// Current playback status
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub state: State,
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
    pub volume: u8,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Crossfade duration in seconds (0 = disabled)
    pub crossfade: u32,
    /// Gapless playback enabled
    pub gapless: bool,
}

/// Playback control trait
pub trait Playback: Send + Sync {
    fn play(&mut self) -> Result<()>;
    fn pause(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<()>;
    fn previous(&mut self) -> Result<()>;
    fn seek(&mut self, position: Duration) -> Result<()>;
    fn seek_relative(&mut self, delta_secs: i64) -> Result<()>;
    fn status(&mut self) -> Result<Status>;

    // =========================================================================
    // Audio Effects (optional - default no-op)
    // =========================================================================

    /// Set crossfade duration in seconds (0 to disable).
    ///
    /// Crossfade blends audio between tracks for smooth transitions.
    /// Check `Capability::Crossfade` before using.
    fn set_crossfade(&mut self, seconds: u32) -> Result<()> {
        let _ = seconds;
        Ok(())
    }

    /// Enable or disable gapless playback.
    ///
    /// Gapless playback removes silence between tracks.
    /// Check `Capability::GaplessPlayback` before using.
    fn set_gapless(&mut self, enabled: bool) -> Result<()> {
        let _ = enabled;
        Ok(())
    }
}

/// Volume control trait
pub trait Volume: Send + Sync {
    fn get(&mut self) -> Result<u8>;
    fn set(&mut self, volume: u8) -> Result<()>;
}

// === Conversions ===

impl From<crate::domain::PlaybackState> for State {
    fn from(s: crate::domain::PlaybackState) -> Self {
        match s {
            crate::domain::PlaybackState::Play => State::Playing,
            crate::domain::PlaybackState::Pause => State::Paused,
            crate::domain::PlaybackState::Stop => State::Stopped,
        }
    }
}
