//! Content discovery traits and types.
//!
//! Search for music, browse directories, and explore the library.

use anyhow::Result;
use super::content::{Item, ContentType};
use crate::domain::ContentDetails;

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

/// Search results
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub items: Vec<Item>,
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
    /// This allows TUI to work uniformly: select anything, resolve, add to queue.
    fn resolve(&mut self, item: &Item) -> Result<Vec<Item>> {
        // Default: track returns itself, others return empty
        if item.is_playable() {
            Ok(vec![item.clone()])
        } else {
            Ok(vec![])
        }
    }

    /// Get detailed view of an item (album, artist, or playlist).
    ///
    /// Returns full content details including tracks, related content,
    /// and metadata. The returned `ContentDetails` enum wraps the
    /// type-specific detail struct.
    ///
    /// # Arguments
    ///
    /// * `item` - The item to get details for. Must be Album, Artist, or Playlist.
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
