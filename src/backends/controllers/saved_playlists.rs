//! Saved playlist operations
//!
//! Create, load, save, and manage persistent playlists.
//! This is different from the playback queue - these are saved playlists
//! that persist across sessions.

use anyhow::Result;

use crate::{
    backends::api::optional::Playlists,
    domain::content::{ContentRef, PlaylistContent},
};

/// Manages saved playlists
///
/// Note: This is for persistent playlists stored on the server,
/// not the current playback queue.
///
/// # Example
///
/// ```ignore
/// if let Some(playlists) = dispatcher.saved_playlists() {
///     // List all saved playlists
///     let all = playlists.list()?;
///     
///     // Get playlist contents
///     let content = playlists.get("favorites")?;
///     
///     // Delete a playlist
///     playlists.delete("old_playlist")?;
/// }
/// ```
pub struct SavedPlaylistController<'a> {
    pub(crate) backend: &'a mut dyn Playlists,
}

impl SavedPlaylistController<'_> {
    /// List all saved playlists
    pub fn list(&mut self) -> Result<Vec<ContentRef>> {
        self.backend.list()
    }

    /// Get the contents of a playlist
    pub fn get(&mut self, name: &str) -> Result<PlaylistContent> {
        self.backend.get(name)
    }

    /// Create a new playlist
    ///
    /// Returns the ID of the created playlist.
    pub fn create(&mut self, name: &str) -> Result<String> {
        self.backend.create(name)
    }

    /// Delete a saved playlist
    pub fn delete(&mut self, name: &str) -> Result<()> {
        self.backend.delete(name)
    }

    /// Rename a saved playlist
    pub fn rename(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.backend.rename(old_name, new_name)
    }

    /// Add tracks to a saved playlist
    pub fn add_tracks(&mut self, playlist: &str, uris: &[String]) -> Result<()> {
        self.backend.add_tracks(playlist, uris)
    }

    /// Remove tracks from a saved playlist by position
    pub fn remove_tracks(&mut self, playlist: &str, positions: &[u32]) -> Result<()> {
        self.backend.remove_tracks(playlist, positions)
    }

    /// Reorder a track within a playlist
    pub fn reorder(&mut self, playlist: &str, from: u32, to: u32) -> Result<()> {
        self.backend.reorder(playlist, from, to)
    }
}
