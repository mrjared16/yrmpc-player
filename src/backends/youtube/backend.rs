//! Legacy YouTube backend - DEPRECATED
//!
//! This file exists only to prevent breaking code that imports YouTubeBackend.
//! The real implementation is YouTubeProxy + YouTubeServer.
//!
//! DO NOT USE THIS. Use YouTubeProxy instead.

use anyhow::Result;

/// Deprecated stub - use YouTubeProxy instead
#[deprecated(
    since = "0.12.0",
    note = "Use YouTubeProxy (client.rs) + YouTubeServer (server/mod.rs) architecture instead. \
            This monolithic backend is no longer maintained."
)]
pub struct YouTubeBackend;

impl YouTubeBackend {
    /// All browse methods return "not implemented"
    pub fn browse_playlist(&self, _id: &str) -> Result<crate::backends::youtube::PlaylistDetails> {
        anyhow::bail!("YouTubeBackend is deprecated. Use YouTubeProxy + implement browse via IPC.")
    }
    
    pub fn browse_album(&self, _id: &str) -> Result<crate::backends::youtube::AlbumDetails> {
        anyhow::bail!("YouTubeBackend is deprecated. Use YouTubeProxy + implement browse via IPC.")
    }
    
    pub fn browse_artist(&self, _id: &str) -> Result<crate::backends::youtube::ArtistDetails> {
        anyhow::bail!("YouTubeBackend is deprecated. Use YouTubeProxy + implement browse via IPC.")
    }
}
