//! Status and information queries
//!
//! Get current playback status, currently playing song, and queue info.

use anyhow::Result;

use crate::backends::api::StatusQuery;
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
    pub(crate) backend: &'a mut dyn StatusQuery,
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
        self.backend.queue_songs()
    }
}
