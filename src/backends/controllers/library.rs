//! Library browsing and search operations
//!
//! Search for music, browse directories, and explore the library.

use anyhow::Result;
use std::time::Duration;

use crate::backends::{MusicBackend, LibraryCategory};
use crate::domain::Song;
use crate::mpd::commands::{LsInfoEntry, Tag};
use crate::mpd::mpd_client::Filter;

/// Browse and search the music library
///
/// # Example
///
/// ```ignore
/// // Search for songs
/// let results = dispatcher.library().search(&[Filter::new(Tag::Any, "beatles")])?;
///
/// // Browse by category
/// let albums = dispatcher.library().by_category(LibraryCategory::Albums)?;
///
/// // Get search suggestions
/// let suggestions = dispatcher.library().suggestions("beat")?;
/// ```
pub struct LibraryBrowser<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

impl LibraryBrowser<'_> {
    /// Search for songs matching the filter
    pub fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>> {
        self.backend.search(filter)
    }

    /// Find songs with optional windowing
    pub fn find(&mut self, filter: &[Filter], window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        self.backend.find(filter, window)
    }

    /// Get search suggestions for autocomplete
    pub fn suggestions(&mut self, query: &str) -> Result<Vec<String>> {
        self.backend.get_search_suggestions(query.to_string())
    }

    /// Browse a directory or path
    ///
    /// For MPD: filesystem path
    /// For YouTube: playlist/album/artist ID with prefix
    pub fn browse(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend.lsinfo(path)
    }

    /// Get library items by category
    ///
    /// Categories: Playlists, Albums, Artists, Songs
    pub fn by_category(&mut self, category: LibraryCategory) -> Result<Vec<LsInfoEntry>> {
        self.backend.get_library(category)
    }

    /// List all items recursively from a path
    pub fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend.list_all(path)
    }

    /// List unique tag values (MPD-specific, returns empty for others)
    ///
    /// Example: list all album names, all artist names, etc.
    pub fn list_tag(&mut self, tag: Tag, filter: Option<&[Filter]>) -> Result<Vec<String>> {
        self.backend.list_tag(tag, filter)
    }

    /// Count songs and total duration matching filter
    pub fn count(&mut self, filter: &[Filter]) -> Result<(usize, Duration)> {
        self.backend.count(filter)
    }
}
