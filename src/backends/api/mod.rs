#![allow(clippy::doc_markdown, clippy::trivially_copy_pass_by_ref)]

//! Backend-agnostic streaming music API.
//!
//! # Purpose
//!
//! These traits define what TUI needs from backends. No MPD types leak through.
//! Backends (YouTube, MPD, future Spotify) implement these traits.
//!
//! # Design Principles
//!
//! 1. **Bulk operations** - `add(&[items])` not `add(&item)` - efficient for
//!    backends
//! 2. **Intent-based** - `InsertAt::Next` not `position: Some(5)` - clear
//!    semantics
//! 3. **Resolvable** - Albums/playlists can expand to tracks via `resolve()`
//! 4. **Minimal** - Only what TUI actually needs
//!
//! # Module Structure
//!
//! - [`playback`] - Playback control (play, pause, seek) and audio effects
//! - [`queue`] - Queue management (add, remove, reorder) and playback behaviors
//! - [`discovery`] - Content discovery (search, browse)
//! - [`content`] - Content types (Item, ContentType, Capability)
//! - [`optional`] - Optional features (Playlists, Lyrics, Radio)
//!
//! # Three-Layer Architecture
//!
//! ```text
//! Layer 1: Universal (api::*) - ALL backends implement
//!   • Playback, Queue, Discovery, Volume
//!
//! Layer 2: Optional Common (api::optional::*) - Multiple backends COULD implement
//!   • Playlists, Lyrics, Radio, UserPreferences
//!
//! Layer 3: Backend-Specific - Only ONE backend has
//!   • mpd::Outputs, mpd::Database, mpd::Stickers
//! ```
//!
//! # For LLM Agents
//!
//! - YouTube: implement these traits, IGNORE mpd/
//! - MPD: implement these traits, IGNORE youtube/

mod content;
mod discovery;
pub mod optional;
mod playback;
mod queue;
mod status_query;

// Re-export all types at api:: level
pub use content::{Capability, ContentType, Item};
pub use discovery::{BrowseResult, Discovery, SearchQuery, SearchResults, SearchSection};
// Re-export optional traits
pub use optional::{Lyrics, Playlists, Radio, UserPreferences};
pub use playback::{Playback, Repeat, State, Status, Volume};
pub use queue::{AfterAdd, InsertAt, Queue, ToggleMode};
pub use status_query::StatusQuery;

// Re-export ContentDetails from domain for convenience
pub use crate::domain::ContentDetails;

/// Complete streaming backend
///
/// Combines all capability traits into one.
pub trait Backend: Playback + Queue + Discovery + Volume {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> &'static [Capability];

    fn supports(&self, cap: Capability) -> bool {
        self.capabilities().contains(&cap)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

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
