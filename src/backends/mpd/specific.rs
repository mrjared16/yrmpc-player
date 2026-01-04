//! MPD-specific traits.
//!
//! These traits define features that ONLY MPD supports. Other backends
//! (YouTube, future Spotify) do not implement these.
//!
//! # Design Principle
//!
//! > Layer 3: Backend-Specific - Only ONE backend has these features.
//!
//! # Usage Pattern
//!
//! UI code should check capability before using:
//!
//! ```ignore
//! if client.supports(Capability::MpdStickers) {
//!     if let Some(stickers) = dispatcher.as_stickers() {
//!         stickers.set("file.mp3", "rating", "5")?;
//!     }
//! }
//! ```
//!
//! # Traits
//!
//! - [`Stickers`] - Metadata tags stored in MPD database
//! - [`Outputs`] - Audio output device control
//! - [`Database`] - Library update/rescan
//!
//! # Removed (Now Universal)
//!
//! The following have been moved to universal traits:
//! - `SavedPlaylists` → [`crate::backends::api::optional::Playlists`]
//! - `Modes` (single/consume/crossfade) → [`crate::backends::api::Queue`] and
//!   [`crate::backends::api::Playback`]

use std::collections::HashMap;

use anyhow::Result;

use crate::mpd::commands::Output;

// =============================================================================
// STICKERS
// =============================================================================

/// MPD stickers - metadata tags stored in the MPD database.
///
/// Stickers allow storing arbitrary key-value pairs associated with songs.
/// Common uses: ratings, play counts, last played time.
///
/// Check `Capability::MpdStickers` before using.
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
// OUTPUTS
// =============================================================================

/// MPD audio outputs - control audio device outputs.
///
/// MPD can have multiple audio outputs (e.g., ALSA, PulseAudio, HTTP stream).
/// This trait allows enabling/disabling individual outputs.
///
/// Check `Capability::MpdOutputs` before using.
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
///
/// Check `Capability::MpdDatabase` before using.
pub trait Database: Send + Sync {
    /// Update the database (scan for new files, fast).
    fn update(&mut self, path: Option<&str>) -> Result<u32>;

    /// Rescan the database (re-read all files, slow but thorough).
    fn rescan(&mut self, path: Option<&str>) -> Result<u32>;
}
