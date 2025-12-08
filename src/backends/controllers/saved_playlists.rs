//! Saved playlist operations (MPD)
//!
//! Create, load, save, and manage persistent playlists.
//! This is different from the playback queue - these are saved playlists
//! that persist across sessions.

use anyhow::Result;

use crate::backends::MusicBackend;
use crate::domain::{Song, QueuePosition};
use crate::mpd::commands::{Playlist, SaveMode};

/// Manages saved playlists (MPD only)
///
/// Note: This is for persistent playlists stored on the MPD server,
/// not the current playback queue.
///
/// # Example
///
/// ```ignore
/// if let Some(playlists) = dispatcher.saved_playlists() {
///     // List all saved playlists
///     let all = playlists.list()?;
///     
///     // Save current queue as a playlist
///     playlists.save("favorites")?;
///     
///     // Load a playlist into the queue
///     playlists.load("favorites", None)?;
/// }
/// ```
pub struct SavedPlaylistController<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

impl SavedPlaylistController<'_> {
    /// List all saved playlists
    pub fn list(&mut self) -> Result<Vec<Playlist>> {
        self.backend.list_playlists()
    }

    /// Get the contents of a playlist
    pub fn contents(&mut self, name: &str) -> Result<Vec<Song>> {
        self.backend.playlist_info_name(name)
    }

    /// Load a playlist into the queue
    ///
    /// # Arguments
    /// * `name` - Playlist name
    /// * `position` - Where to insert in queue (None = end)
    pub fn load(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()> {
        self.backend.load_playlist(name, position)
    }

    /// Save the current queue as a playlist
    pub fn save(&mut self, name: &str) -> Result<()> {
        self.backend.save_queue_as_playlist(name, Some(SaveMode::Create))
    }

    /// Save with mode (create, append, replace)
    pub fn save_with_mode(&mut self, name: &str, mode: SaveMode) -> Result<()> {
        self.backend.save_queue_as_playlist(name, Some(mode))
    }

    /// Delete a saved playlist
    pub fn delete(&mut self, name: &str) -> Result<()> {
        self.backend.delete_playlist(name)
    }

    /// Rename a saved playlist
    pub fn rename(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.backend.rename_playlist(old_name, new_name)
    }

    /// Add a song to a saved playlist
    pub fn add_song(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.backend.add_to_playlist(playlist, uri)
    }

    /// Remove a song from a saved playlist by position
    pub fn remove_song(&mut self, playlist: &str, position: u32) -> Result<()> {
        self.backend.delete_from_playlist(playlist, position)
    }
}
