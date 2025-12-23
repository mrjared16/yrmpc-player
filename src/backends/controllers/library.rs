//! Library browsing and search operations
//!
//! Search for music, browse directories, and explore the library.

use anyhow::Result;

use crate::backends::api::{Discovery as DiscoveryTrait, Item, SearchQuery, SearchResults, BrowseResult};

/// Browse and search the music library
///
/// # Example
///
/// ```ignore
/// // Search for content
/// let results = dispatcher.library().search("beatles")?;
///
/// // Browse a path
/// let items = dispatcher.library().browse("/")?;
///
/// // Get search suggestions
/// let suggestions = dispatcher.library().suggestions("beat")?;
/// ```
pub struct LibraryBrowser<'a> {
    pub(crate) backend: &'a mut dyn DiscoveryTrait,
}

impl LibraryBrowser<'_> {
    /// Search for content matching the query
    pub fn search(&mut self, query: impl Into<String>) -> Result<SearchResults> {
        self.backend.search(SearchQuery::new(query))
    }

    /// Search with full query options
    pub fn search_query(&mut self, query: SearchQuery) -> Result<SearchResults> {
        self.backend.search(query)
    }

    /// Get search suggestions for autocomplete
    pub fn suggestions(&mut self, partial: &str) -> Result<Vec<String>> {
        self.backend.suggestions(partial)
    }

    /// Browse a directory or path
    ///
    /// For MPD: filesystem path
    /// For YouTube: playlist/album/artist ID with prefix
    pub fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        self.backend.browse(path)
    }

    /// Browse root directory
    pub fn browse_root(&mut self) -> Result<BrowseResult> {
        self.backend.browse("")
    }

    /// Resolve an item to playable tracks
    ///
    /// - Track → returns itself
    /// - Album → returns album tracks
    /// - Playlist → returns playlist tracks
    /// - Artist → returns top tracks
    pub fn resolve(&mut self, item: &Item) -> Result<Vec<Item>> {
        self.backend.resolve(item)
    }
}
