//! Volume control operations
//!
//! Get, set, and adjust playback volume.

use anyhow::Result;

use crate::backends::MusicBackend;
use crate::mpd::commands::ValueChange;

/// Controls playback volume
///
/// # Example
///
/// ```ignore
/// dispatcher.volume().set(80)?;
/// dispatcher.volume().adjust(-10)?; // decrease by 10
/// let current = dispatcher.volume().get()?;
/// ```
pub struct VolumeController<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

impl VolumeController<'_> {
    /// Get current volume level (0-100)
    pub fn get(&mut self) -> Result<u8> {
        self.backend.volume()
    }

    /// Set absolute volume level (0-100)
    pub fn set(&mut self, level: u8) -> Result<()> {
        self.backend.set_volume(ValueChange::Set(level.min(100) as u32))
    }

    /// Adjust volume relatively
    ///
    /// Positive values increase, negative values decrease.
    pub fn adjust(&mut self, delta: i8) -> Result<()> {
        if delta >= 0 {
            self.backend.set_volume(ValueChange::Increase(delta as u32))
        } else {
            self.backend.set_volume(ValueChange::Decrease((-delta) as u32))
        }
    }

    /// Increase volume by amount
    pub fn up(&mut self, amount: u8) -> Result<()> {
        self.backend.set_volume(ValueChange::Increase(amount as u32))
    }

    /// Decrease volume by amount
    pub fn down(&mut self, amount: u8) -> Result<()> {
        self.backend.set_volume(ValueChange::Decrease(amount as u32))
    }

    /// Mute (set to 0)
    pub fn mute(&mut self) -> Result<()> {
        self.set(0)
    }
}
