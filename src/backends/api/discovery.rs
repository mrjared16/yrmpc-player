#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use
)]

//! Content discovery traits and types.
//!
//! Search for music, browse directories, and explore the library.

use anyhow::Result;

use super::content::{ContentType, Item};
use crate::domain::content::ContentDetails;

/// Search query
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: String,
    pub filter: Option<ContentType>,
    pub limit: Option<u32>,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }

    pub fn with_filter(mut self, filter: ContentType) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Search results with structured sections
///
/// Sections contain their items as containers, not markers in a flat list.
/// This enables proper separation of concerns:
/// - Backend: Returns sections in native order
/// - UI: Applies config-based ordering (presentation concern)
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    /// Sections of search results (e.g., "Songs", "Albums", "Artists")
    pub sections: Vec<SearchSection>,
}

/// A section of search results
#[derive(Debug, Clone, Default)]
pub struct SearchSection {
    /// Section key for config ordering (e.g., "songs", "albums", "top_results")
    pub key: String,
    /// Display title (e.g., "Songs", "Albums", "Top Result")
    pub title: String,
    /// Items in this section
    pub items: Vec<Item>,
}

impl SearchSection {
    pub fn new(key: impl Into<String>, title: impl Into<String>, items: Vec<Item>) -> Self {
        Self { key: key.into(), title: title.into(), items }
    }
}

impl SearchResults {
    pub fn new() -> Self {
        Self { sections: Vec::new() }
    }

    pub fn add_section(&mut self, section: SearchSection) {
        self.sections.push(section);
    }

    /// Total number of items across all sections
    pub fn total_items(&self) -> usize {
        self.sections.iter().map(|s| s.items.len()).sum()
    }

    /// Flatten all sections into a single list of items (loses section
    /// structure)
    #[deprecated(note = "Use sections directly to preserve structure")]
    pub fn items(&self) -> Vec<Item> {
        self.sections.iter().flat_map(|s| s.items.clone()).collect()
    }
}

/// Browse results with navigation context
#[derive(Debug, Clone, Default)]
pub struct BrowseResult {
    /// Current path
    pub path: String,
    /// Items at this path
    pub items: Vec<Item>,
    /// Parent path for navigation (None if at root)
    pub parent: Option<String>,
}

/// Content discovery trait
pub trait Discovery: Send + Sync {
    /// Search for content
    fn search(&mut self, query: SearchQuery) -> Result<SearchResults>;

    /// Browse a path (directories, categories)
    fn browse(&mut self, path: &str) -> Result<BrowseResult>;

    /// Get search suggestions
    fn suggestions(&mut self, partial: &str) -> Result<Vec<String>>;

    /// Resolve non-track item to playable tracks
    ///
    /// - Track → returns itself
    /// - Album → returns album tracks
    /// - Playlist → returns playlist tracks
    /// - Artist → returns top/all tracks
    ///
    /// This allows TUI to work uniformly: select anything, resolve, add to
    /// queue.
    fn resolve(&mut self, item: &Item) -> Result<Vec<Item>> {
        // Default: track returns itself, others return empty
        if item.is_playable() { Ok(vec![item.clone()]) } else { Ok(vec![]) }
    }

    /// Get detailed view of an item (album, artist, or playlist).
    ///
    /// Returns full content details including tracks, related content,
    /// and metadata. The returned `ContentDetails` enum wraps the
    /// type-specific detail struct.
    ///
    /// # Arguments
    ///
    /// * `item` - The item to get details for. Must be Album, Artist, or
    ///   Playlist.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The item type doesn't support details (e.g., Track)
    /// - The backend fails to fetch the data
    /// - The item ID is invalid
    ///
    /// # Example
    ///
    /// ```ignore
    /// match backend.details(&album_item)? {
    ///     ContentDetails::Album(album) => {
    ///         println!("Album: {} by {}", album.title, album.artist.name);
    ///         for track in &album.tracks {
    ///             println!("  - {}", track.title());
    ///         }
    ///     }
    ///     _ => {}
    /// }
    /// ```
    fn details(&mut self, item: &Item) -> Result<ContentDetails>;
}
