//! Rich status query trait.
//!
//! This trait provides access to full player status including
//! queue state, modes, and current song information.

use anyhow::Result;

use crate::domain::{Song, Status};

/// Rich status query trait.
///
/// Unlike [`super::Playback::status()`] which returns minimal status
/// (state, position, volume), this trait returns the full [`domain::Status`]
/// with all available information for UI display.
///
/// # When to Use
///
/// - Use `api::Playback::status()` for basic playback state
/// - Use `api::StatusQuery` for full UI status (queue position, modes, etc.)
///
/// # Example
///
/// ```ignore
/// let status = backend.get_status()?;
/// if let Some(song_id) = status.songid {
///     // Highlight current song in queue
/// }
/// if status.updating_db.is_some() {
///     // Show database update indicator
/// }
/// ```
pub trait StatusQuery: Send + Sync {
    /// Get full player status.
    ///
    /// Returns rich status including:
    /// - Playback state, volume
    /// - Current song ID and position
    /// - Queue version and length
    /// - Single/consume/repeat/random modes
    /// - Database update status
    /// - Error messages
    fn get_status(&mut self) -> Result<Status>;

    /// Get the currently playing song.
    ///
    /// Returns `None` if nothing is playing.
    fn current_song(&mut self) -> Result<Option<Song>>;

    /// Get the current queue as domain songs.
    ///
    /// This returns full `domain::Song` objects for rich display.
    /// For simpler `api::Item` list, use `api::Queue::list()`.
    fn queue_songs(&mut self) -> Result<Vec<Song>>;
}
