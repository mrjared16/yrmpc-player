#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

//! Optional common traits for features multiple backends COULD support.
//!
//! These traits define features that are universal concepts but not every
//! backend may implement them. UI checks `Capability` flags before using.
//!
//! # Design Principle
//!
//! > "Ask CAN you do X, not ARE you backend Y"
//!
//! ```ignore
//! // ✅ CORRECT
//! if client.supports(Capability::Playlists) {
//!     let playlists = client.playlists().list()?;
//! }
//!
//! // ❌ WRONG
//! if let Some(mpd) = client.as_mpd() {
//!     let playlists = mpd.list_playlists()?;
//! }
//! ```
//!
//! # Available Traits
//!
//! - [`Playlists`] - User library playlists (MPD, YouTube, Spotify)
//! - [`Lyrics`] - Song lyrics
//! - [`Radio`] - Seed-based recommendations
//! - [`UserPreferences`] - Likes/dislikes

mod playlists;

use anyhow::Result;
pub use playlists::Playlists;

// =============================================================================
// LYRICS
// =============================================================================

/// Song lyrics retrieval.
///
/// Check `Capability::Lyrics` before using.
pub trait Lyrics: Send + Sync {
    /// Get lyrics for a track.
    fn get_lyrics(&mut self, track_id: &str) -> Result<Option<String>> {
        let _ = track_id;
        Ok(None)
    }
}

// =============================================================================
// RADIO
// =============================================================================

/// Seed-based recommendations (YouTube Mix, Spotify Radio).
///
/// Check `Capability::Radio` before using.
pub trait Radio: Send + Sync {
    /// Start a radio/mix based on a seed track.
    ///
    /// This typically replaces the current queue with recommended tracks.
    fn start_radio(&mut self, seed_track_id: &str) -> Result<()> {
        let _ = seed_track_id;
        anyhow::bail!("Radio not supported by this backend")
    }
}

// =============================================================================
// USER PREFERENCES
// =============================================================================

/// User preferences like likes/dislikes.
///
/// Check `Capability::UserLikes` before using.
pub trait UserPreferences: Send + Sync {
    /// Check if a track is liked.
    fn is_liked(&mut self, track_id: &str) -> Result<bool> {
        let _ = track_id;
        Ok(false)
    }

    /// Set like status for a track.
    fn set_liked(&mut self, track_id: &str, liked: bool) -> Result<()> {
        let _ = (track_id, liked);
        anyhow::bail!("User preferences not supported by this backend")
    }
}
