//! Volume control operations
//!
//! Get, set, and adjust playback volume.

use anyhow::Result;

use crate::backends::api::Volume as VolumeTrait;

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
    pub(crate) backend: &'a mut dyn VolumeTrait,
}

impl VolumeController<'_> {
    /// Get current volume level (0-100)
    pub fn get(&mut self) -> Result<u8> {
        self.backend.get()
    }

    /// Set absolute volume level (0-100)
    pub fn set(&mut self, level: u8) -> Result<()> {
        self.backend.set(level.min(100))
    }

    /// Adjust volume relatively
    ///
    /// Positive values increase, negative values decrease.
    pub fn adjust(&mut self, delta: i8) -> Result<()> {
        let current = self.get()?;
        let new_level = if delta >= 0 {
            current.saturating_add(delta as u8).min(100)
        } else {
            current.saturating_sub((-delta) as u8)
        };
        self.set(new_level)
    }

    /// Increase volume by amount
    pub fn up(&mut self, amount: u8) -> Result<()> {
        let current = self.get()?;
        self.set(current.saturating_add(amount).min(100))
    }

    /// Decrease volume by amount
    pub fn down(&mut self, amount: u8) -> Result<()> {
        let current = self.get()?;
        self.set(current.saturating_sub(amount))
    }

    /// Mute (set to 0)
    pub fn mute(&mut self) -> Result<()> {
        self.set(0)
    }
}
