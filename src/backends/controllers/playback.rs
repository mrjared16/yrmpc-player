//! Playback control operations
//!
//! Controls the playback state: play, pause, stop, seek, next, previous.

use anyhow::Result;
use std::time::Duration;

use crate::backends::MusicBackend;
use crate::mpd::commands::SeekPosition;

/// Controls playback state
///
/// # Example
///
/// ```ignore
/// dispatcher.playback().play()?;
/// dispatcher.playback().seek(SeekMode::Absolute(Duration::from_secs(30)))?;
/// ```
pub struct PlaybackController<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

/// Seek mode for playback position
#[derive(Debug, Clone, Copy)]
pub enum SeekMode {
    /// Seek to absolute position from start
    Absolute(Duration),
    /// Seek relative to current position (positive = forward, negative = backward)
    Relative(f64),
}

impl PlaybackController<'_> {
    /// Start or resume playback
    pub fn play(&mut self) -> Result<()> {
        self.backend.play()
    }

    /// Pause playback
    pub fn pause(&mut self) -> Result<()> {
        self.backend.pause(true)
    }

    /// Resume playback (unpause)
    pub fn resume(&mut self) -> Result<()> {
        self.backend.pause(false)
    }

    /// Toggle between play and pause
    pub fn toggle(&mut self) -> Result<()> {
        let status = self.backend.get_status()?;
        match status.state {
            crate::domain::PlaybackState::Play => self.pause(),
            crate::domain::PlaybackState::Pause => self.resume(),
            crate::domain::PlaybackState::Stop => self.play(),
        }
    }

    /// Stop playback
    pub fn stop(&mut self) -> Result<()> {
        self.backend.stop()
    }

    /// Skip to next track
    pub fn next(&mut self) -> Result<()> {
        self.backend.next()
    }

    /// Go to previous track
    pub fn previous(&mut self) -> Result<()> {
        self.backend.previous()
    }

    /// Seek to a position
    pub fn seek(&mut self, mode: SeekMode) -> Result<()> {
        let position = match mode {
            SeekMode::Absolute(duration) => SeekPosition::Absolute(duration.as_secs_f64()),
            SeekMode::Relative(delta) => SeekPosition::Relative(delta),
        };
        self.backend.seek_current(position)
    }

    /// Seek to absolute position in seconds
    pub fn seek_to(&mut self, seconds: f64) -> Result<()> {
        self.backend.seek_current(SeekPosition::Absolute(seconds))
    }

    /// Seek relative to current position
    pub fn seek_by(&mut self, delta: f64) -> Result<()> {
        self.backend.seek_current(SeekPosition::Relative(delta))
    }
}
