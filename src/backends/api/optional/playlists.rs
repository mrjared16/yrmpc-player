#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

//! User library playlists trait.
//!
//! This trait abstracts playlist operations that both MPD and YouTube support:
//! - MPD: Local `.m3u` files with full CRUD
//! - YouTube: Remote playlists via API, 2-way sync with account
//!
//! The TUI doesn't need to know the difference - same interface, different
//! implementations.

use anyhow::Result;

use crate::domain::content::{ContentRef, PlaylistContent};

/// User library playlists.
///
/// Check `Capability::Playlists` before using.
///
/// # Capability Flags
///
/// - `Capability::Playlists` - Can list and view playlists
/// - `Capability::PlaylistCreate` - Can create new playlists
/// - `Capability::PlaylistEdit` - Can modify existing playlists
///
/// # Example
///
/// ```ignore
/// if client.supports(Capability::Playlists) {
///     let playlists = client.playlists()?.list()?;
///     for playlist in playlists {
///         println!("{}: {}", playlist.id, playlist.name);
///     }
/// }
/// ```
pub trait Playlists: Send + Sync {
    /// List all user playlists.
    fn list(&mut self) -> Result<Vec<ContentRef>>;

    /// Get full playlist details including tracks.
    fn get(&mut self, id: &str) -> Result<PlaylistContent>;

    /// Create a new playlist.
    ///
    /// Returns the ID of the created playlist.
    /// Check `Capability::PlaylistCreate` before calling.
    fn create(&mut self, name: &str) -> Result<String> {
        let _ = name;
        anyhow::bail!("Playlist creation not supported by this backend")
    }

    /// Delete a playlist.
    ///
    /// Check `Capability::PlaylistEdit` before calling.
    fn delete(&mut self, id: &str) -> Result<()> {
        let _ = id;
        anyhow::bail!("Playlist deletion not supported by this backend")
    }

    /// Rename a playlist.
    ///
    /// Check `Capability::PlaylistEdit` before calling.
    fn rename(&mut self, id: &str, new_name: &str) -> Result<()> {
        let _ = (id, new_name);
        anyhow::bail!("Playlist rename not supported by this backend")
    }

    /// Add tracks to a playlist.
    ///
    /// Check `Capability::PlaylistEdit` before calling.
    fn add_tracks(&mut self, playlist_id: &str, track_ids: &[String]) -> Result<()> {
        let _ = (playlist_id, track_ids);
        anyhow::bail!("Adding to playlist not supported by this backend")
    }

    /// Remove tracks from a playlist by position.
    ///
    /// Check `Capability::PlaylistEdit` before calling.
    fn remove_tracks(&mut self, playlist_id: &str, positions: &[u32]) -> Result<()> {
        let _ = (playlist_id, positions);
        anyhow::bail!("Removing from playlist not supported by this backend")
    }

    /// Reorder a track within a playlist.
    ///
    /// Check `Capability::PlaylistEdit` before calling.
    fn reorder(&mut self, playlist_id: &str, from: u32, to: u32) -> Result<()> {
        let _ = (playlist_id, from, to);
        anyhow::bail!("Playlist reorder not supported by this backend")
    }
}
