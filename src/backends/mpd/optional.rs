//! MPD-specific optional traits.
//!
//! These traits define features that ONLY MPD supports. Other backends
//! (YouTube, future Spotify) do not implement these.
//!
//! # Usage Pattern
//!
//! UI code should check for availability before using:
//!
//! ```ignore
//! if let Some(stickers) = dispatcher.as_stickers() {
//!     stickers.set("file.mp3", "rating", "5")?;
//! }
//! ```
//!
//! # Traits
//!
//! - [`Stickers`] - Metadata tags stored in MPD database
//! - [`SavedPlaylists`] - Persistent playlist storage
//! - [`Outputs`] - Audio output device control
//! - [`Database`] - Library update/rescan
//! - [`Modes`] - MPD-specific playback modes (single, consume, crossfade)

use std::collections::HashMap;

use anyhow::Result;

use crate::{
    domain::{QueuePosition, Song},
    mpd::{
        SingleOrRange,
        commands::{OnOffOneshot, Output, Playlist, SaveMode},
    },
};

// =============================================================================
// STICKERS
// =============================================================================

/// MPD stickers - metadata tags stored in the MPD database.
///
/// Stickers allow storing arbitrary key-value pairs associated with songs.
/// Common uses: ratings, play counts, last played time.
pub trait Stickers: Send + Sync {
    /// List all stickers for a URI.
    fn list(&mut self, uri: &str) -> Result<HashMap<String, String>>;

    /// Get a specific sticker value.
    fn get(&mut self, uri: &str, key: &str) -> Result<Option<String>> {
        Ok(self.list(uri)?.get(key).cloned())
    }

    /// Set a sticker value.
    fn set(&mut self, uri: &str, key: &str, value: &str) -> Result<()>;

    /// Delete a sticker.
    fn delete(&mut self, uri: &str, key: &str) -> Result<()>;

    /// Delete all stickers for a URI.
    fn delete_all(&mut self, uri: &str) -> Result<()> {
        for key in self.list(uri)?.keys() {
            self.delete(uri, key)?;
        }
        Ok(())
    }
}

// =============================================================================
// SAVED PLAYLISTS
// =============================================================================

/// MPD saved playlists - persistent playlist storage in MPD.
///
/// Unlike the queue (current playlist), saved playlists persist across
/// server restarts and can be loaded/saved by name.
pub trait SavedPlaylists: Send + Sync {
    /// List all saved playlists.
    fn list(&mut self) -> Result<Vec<Playlist>>;

    /// Get songs in a saved playlist.
    fn get(&mut self, name: &str) -> Result<Vec<Song>>;

    /// Load a saved playlist into the queue.
    fn load(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()>;

    /// Save current queue as a playlist.
    fn save(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()>;

    /// Delete a saved playlist.
    fn delete(&mut self, name: &str) -> Result<()>;

    /// Rename a saved playlist.
    fn rename(&mut self, old_name: &str, new_name: &str) -> Result<()>;

    /// Add a song to a saved playlist.
    fn add_song(&mut self, playlist: &str, uri: &str) -> Result<()>;

    /// Remove a song from a saved playlist by position.
    fn remove_song(&mut self, playlist: &str, position: u32) -> Result<()>;

    /// Move songs within a saved playlist.
    fn move_songs(&mut self, playlist: &str, from: SingleOrRange, to: u32) -> Result<()>;
}

// =============================================================================
// OUTPUTS
// =============================================================================

/// MPD audio outputs - control audio device outputs.
///
/// MPD can have multiple audio outputs (e.g., ALSA, PulseAudio, HTTP stream).
/// This trait allows enabling/disabling individual outputs.
pub trait Outputs: Send + Sync {
    /// List all audio outputs.
    fn list(&mut self) -> Result<Vec<Output>>;

    /// Enable an output by ID.
    fn enable(&mut self, id: u32) -> Result<()>;

    /// Disable an output by ID.
    fn disable(&mut self, id: u32) -> Result<()>;

    /// Toggle an output by ID.
    fn toggle(&mut self, id: u32) -> Result<()>;
}

// =============================================================================
// DATABASE
// =============================================================================

/// MPD database management - library update and rescan.
///
/// MPD maintains a database of all music files. These operations
/// trigger MPD to scan for new/changed files.
pub trait Database: Send + Sync {
    /// Update the database (scan for new files, fast).
    fn update(&mut self, path: Option<&str>) -> Result<u32>;

    /// Rescan the database (re-read all files, slow but thorough).
    fn rescan(&mut self, path: Option<&str>) -> Result<u32>;
}

// =============================================================================
// MODES
// =============================================================================

/// MPD-specific playback modes.
///
/// These modes are specific to MPD's playback behavior:
/// - `single`: Play only one song then stop/pause
/// - `consume`: Remove songs from queue after playing
/// - `crossfade`: Crossfade between tracks
pub trait Modes: Send + Sync {
    /// Set single mode (play one song then stop).
    fn set_single(&mut self, mode: OnOffOneshot) -> Result<()>;

    /// Set consume mode (remove songs after playing).
    fn set_consume(&mut self, mode: OnOffOneshot) -> Result<()>;

    /// Set crossfade duration in seconds.
    fn set_crossfade(&mut self, seconds: u32) -> Result<()>;

    /// Shuffle a range of the queue.
    fn shuffle_range(&mut self, range: Option<SingleOrRange>) -> Result<()>;
}
