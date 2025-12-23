//! Backend-agnostic streaming music API.
//!
//! # Purpose
//!
//! These traits define what TUI needs from backends. No MPD types leak through.
//! Backends (YouTube, MPD, future Spotify) implement these traits.
//!
//! # Design Principles
//!
//! 1. **Bulk operations** - `add(&[items])` not `add(&item)` - efficient for backends
//! 2. **Intent-based** - `InsertAt::Next` not `position: Some(5)` - clear semantics
//! 3. **Resolvable** - Albums/playlists can expand to tracks via `resolve()`
//! 4. **Minimal** - Only what TUI actually needs
//!
//! # Module Structure
//!
//! - [`playback`] - Playback control (play, pause, seek) and volume
//! - [`queue`] - Queue management (add, remove, reorder)
//! - [`discovery`] - Content discovery (search, browse)
//! - [`content`] - Content types (Item, ContentType, Capability)
//!
//! # Adding a New Backend (e.g., Spotify)
//!
//! 1. **Create backend module**: `src/backends/spotify/`
//! 2. **Implement api traits**:
//!    - [`Playback`] - play, pause, stop, seek, status (~8 methods)
//!    - [`Queue`] - add, remove, clear, move_items (~8 methods)  
//!    - [`Discovery`] - search, browse, suggestions (~4 methods)
//!    - [`Volume`] - get, set (~2 methods)
//!    - [`Backend`] - name, capabilities (~2 methods)
//!
//! 3. **Add to BackendDispatcher** (`src/backends/client.rs`):
//!    ```ignore
//!    pub enum BackendDispatcher<'a> {
//!        Mpd(MpdBackend<'a>),
//!        YouTube(YouTubeProxy),
//!        Spotify(SpotifyClient),  // Add new variant
//!    }
//!    ```
//!
//! 4. **Implement api:: trait impls** for BackendDispatcher (update match arms)
//!
//! 5. **Add Spotify-specific features** (optional):
//!    - Add `pub fn spotify(&mut self) -> Option<&mut SpotifyClient>` to BackendDispatcher
//!    - UI can use for Spotify-only features like recommendations
//!
//! Total: ~24 required methods, most are simple pass-throughs.
//! 
//! # For LLM Agents
//!
//! - YouTube: implement these traits, IGNORE mpd/
//! - MPD: implement these traits, IGNORE youtube/

mod content;
mod discovery;
mod playback;
mod queue;

// Re-export all types at api:: level
pub use content::{ContentType, Item, Capability};
pub use discovery::{Discovery, SearchQuery, SearchResults, BrowseResult};
pub use playback::{Playback, Volume, State, Status, Repeat};
pub use queue::{Queue, InsertAt, AfterAdd};

// Re-export ContentDetails from domain for convenience
pub use crate::domain::ContentDetails;

/// Complete streaming backend
///
/// Combines all capability traits into one.
pub trait Backend: Playback + Queue + Discovery + Volume {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> &[Capability];

    fn supports(&self, cap: Capability) -> bool {
        self.capabilities().contains(&cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_item_builder() {
        let item = Item::track("id123", "Song Title")
            .with_artist("Artist")
            .with_duration(Duration::from_secs(180));

        assert_eq!(item.id, "id123");
        assert_eq!(item.title, "Song Title");
        assert!(item.is_playable());
        assert!(!item.needs_resolve());
    }

    #[test]
    fn test_album_needs_resolve() {
        let album = Item {
            id: "album123".into(),
            content_type: ContentType::Album,
            title: "Album Name".into(),
            ..Default::default()
        };

        assert!(!album.is_playable());
        assert!(album.needs_resolve());
    }
}
