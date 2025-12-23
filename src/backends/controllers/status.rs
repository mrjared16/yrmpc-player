//! Status and information queries
//!
//! Get current playback status, currently playing song, and queue info.
//!
//! # Why This Uses `MusicBackend` Instead of `api::Playback`
//!
//! This controller returns [`domain::Status`] (rich, full-featured) rather than
//! [`api::Status`] (minimal). The UI needs fields like:
//! - `songid` / `next_songid` - for queue highlighting
//! - `playlist` version - for change detection  
//! - `consume` / `single` modes - for mode display
//! - `updating_db` - for database update indicator
//!
//! These are MPD-specific but useful for rich UI display. The YouTube backend
//! provides stubs for these fields.
//!
//! For minimal status (just state/position/volume), use [`api::Playback::status()`]
//! via [`PlaybackController`].

use anyhow::Result;

use crate::backends::MusicBackend;
use crate::domain::{Song, Status};

/// Provides playback status and song information
///
/// # Example
///
/// ```ignore
/// let status = dispatcher.status().get()?;
/// if let Some(song) = dispatcher.status().current_song()? {
///     println!("Now playing: {}", song.title.unwrap_or_default());
/// }
/// ```
pub struct StatusProvider<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

impl StatusProvider<'_> {
    /// Get current playback status
    ///
    /// Returns information about playback state, current position,
    /// volume, repeat/random modes, etc.
    pub fn get(&mut self) -> Result<Status> {
        self.backend.get_status()
    }

    /// Get the currently playing song
    ///
    /// Returns `None` if nothing is playing.
    pub fn current_song(&mut self) -> Result<Option<Song>> {
        self.backend.current_song()
    }

    /// Get the current queue (alias for queue().list())
    pub fn queue(&mut self) -> Result<Vec<Song>> {
        self.backend.playlist_info()
    }
}
