//! Search result types for YouTube Music
//!
//! This module provides type-safe representation of search results,
//! separating playable items (songs/videos) from browsable items (artists/albums/playlists).
//!
//! Design rationale: See docs/design-choices.md

mod items;
mod display;
mod convert;

pub use items::*;
pub use display::Displayable;

use std::time::Duration;

/// Top-level search result type
///
/// Separates playable vs browsable to prevent LSP violations
/// in queue operations (Artist can't be queued).
///
/// NOTE: Headers are NOT part of domain model - they belong in UI layer.
/// Use ListItem::Header for section separators in UI lists.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchItem {
    /// Songs and videos - can be queued directly
    Playable(PlayableItem),
    /// Artists, albums, playlists - open detail view
    Browsable(BrowsableItem),
}

/// Items that can be queued directly (have video_id)
#[derive(Debug, Clone, PartialEq)]
pub enum PlayableItem {
    Song(SongItem),
    Video(VideoItem),
}

/// Items that open a detail view on Enter
#[derive(Debug, Clone, PartialEq)]
pub enum BrowsableItem {
    Artist(ArtistItem),
    Album(AlbumItem),
    Playlist(PlaylistItem),
}

/// Action triggered by Enter key
#[derive(Debug, Clone, PartialEq)]
pub enum ItemAction {
    /// Play this video_id immediately
    Play(String),
    /// Navigate to this path (e.g., "artist:UC123")
    Browse(String),
}

/// Reference to content that needs fetching for queue
#[derive(Debug, Clone, PartialEq)]
pub enum ContentRef {
    Album(String),
    Playlist(String),
}

/// Queue capability - determines if/how item can be queued
#[derive(Debug, Clone, PartialEq)]
pub enum QueueCapability {
    /// Single item, queue immediately with this video_id
    Direct(String),
    /// Needs API fetch to get tracks
    Fetchable(ContentRef),
}

/// Type of queue operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueAction {
    /// Insert after current song
    PlayNext,
    /// Append to end of queue
    PlayLast,
    /// Clear queue, insert, and start playback
    PlayNow,
}

/// A section of search results with a title header
///
/// This preserves the API's section structure (e.g., "Top Result", "Songs", "Artists")
/// without mixing header metadata into the domain item types.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchSection {
    /// Section key for config ordering (e.g., "top_results", "songs", "artists")
    pub key: String,
    /// Section title for display (e.g., "Top Result", "Songs", "Artists")
    pub title: String,
    /// Items in this section
    pub items: Vec<SearchItem>,
}

impl SearchSection {
    /// Create a new search section
    pub fn new(key: impl Into<String>, title: impl Into<String>, items: Vec<SearchItem>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            items,
        }
    }

    /// Check if this section is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get the number of items in this section
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Complete search results containing multiple sections
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResults {
    /// Sections of search results
    pub sections: Vec<SearchSection>,
}

impl SearchResults {
    /// Create empty search results
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a section to the results
    pub fn add_section(&mut self, section: SearchSection) {
        if !section.is_empty() {
            self.sections.push(section);
        }
    }

    /// Flatten all items from all sections into a single Vec
    /// (for backwards compatibility with code expecting Vec<SearchItem>)
    pub fn flatten(&self) -> Vec<SearchItem> {
        self.sections
            .iter()
            .flat_map(|s| s.items.clone())
            .collect()
    }

    /// Get total item count across all sections
    pub fn total_items(&self) -> usize {
        self.sections.iter().map(|s| s.items.len()).sum()
    }

    /// Check if there are any results
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() || self.sections.iter().all(|s| s.is_empty())
    }
}

// ============ Implementations ============

impl SearchItem {
    /// Get the action for Enter key
    pub fn action(&self) -> ItemAction {
        match self {
            Self::Playable(p) => ItemAction::Play(p.video_id().to_string()),
            Self::Browsable(b) => ItemAction::Browse(b.browse_path()),
        }
    }
}

impl PlayableItem {
    /// Get the video ID for playback
    pub fn video_id(&self) -> &str {
        match self {
            Self::Song(s) => &s.video_id,
            Self::Video(v) => &v.video_id,
        }
    }

    /// Queue capability - always Direct for playable items
    pub fn queue_capability(&self) -> QueueCapability {
        QueueCapability::Direct(self.video_id().to_string())
    }

    /// Convert to domain Song for queue operations
    pub fn to_song(&self) -> crate::domain::Song {
        match self {
            Self::Song(s) => s.into(),
            Self::Video(v) => v.into(),
        }
    }
}

impl BrowsableItem {
    /// Get the browse path (e.g., "artist:UC123")
    pub fn browse_path(&self) -> String {
        match self {
            Self::Artist(a) => {
                if let Some(ref id) = a.browse_id {
                    format!("artist:{}", id)
                } else {
                    // Fallback for artists without browse_id
                    format!("artist:name:{}", a.name.replace(' ', "_"))
                }
            }
            Self::Album(a) => format!("album:{}", a.album_id),
            Self::Playlist(p) => format!("playlist:{}", p.playlist_id),
        }
    }

    /// Check if this item can be queued (Albums/Playlists can, Artists cannot)
    pub fn can_queue(&self) -> bool {
        !matches!(self, Self::Artist(_))
    }

    /// Get content reference for fetching tracks (None for Artist)
    pub fn content_ref(&self) -> Option<ContentRef> {
        match self {
            Self::Artist(_) => None,
            Self::Album(a) => Some(ContentRef::Album(a.album_id.clone())),
            Self::Playlist(p) => Some(ContentRef::Playlist(p.playlist_id.clone())),
        }
    }

    /// Queue capability if this item can be queued
    pub fn queue_capability(&self) -> Option<QueueCapability> {
        self.content_ref().map(QueueCapability::Fetchable)
    }
}

// ============ Conversions from API types ============

/// Convert api::Item to SearchItem
impl From<crate::backends::api::Item> for SearchItem {
    fn from(item: crate::backends::api::Item) -> Self {
        use crate::domain::ContentType;
        use items::{SongItem, VideoItem, ArtistItem, AlbumItem, PlaylistItem};

        match item.content_type {
            ContentType::Track => SearchItem::Playable(PlayableItem::Song(SongItem {
                video_id: item.id,
                title: item.title,
                artist: item.subtitle.unwrap_or_default(),
                album: None,
                duration: item.duration,
                thumbnail: item.thumbnail,
                explicit: false,
            })),
            ContentType::Artist => SearchItem::Browsable(BrowsableItem::Artist(ArtistItem {
                name: item.title,
                browse_id: Some(item.id),
                thumbnail: item.thumbnail,
                subscribers: item.subtitle,
            })),
            ContentType::Album => SearchItem::Browsable(BrowsableItem::Album(AlbumItem {
                album_id: item.id,
                title: item.title,
                artist: item.subtitle.unwrap_or_default(),
                year: None,
                album_type: None,
                thumbnail: item.thumbnail,
                explicit: false,
            })),
            ContentType::Playlist => SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                playlist_id: item.id,
                title: item.title,
                author: item.subtitle.unwrap_or_default(),
                track_count: None,
                thumbnail: item.thumbnail,
            })),
            // Directory and other types default to video (can't be easily mapped)
            _ => SearchItem::Playable(PlayableItem::Video(VideoItem {
                video_id: item.id,
                title: item.title,
                channel: item.subtitle.unwrap_or_default(),
                views: None,
                duration: item.duration,
                thumbnail: item.thumbnail,
            })),
        }
    }
}

/// Convert api::SearchSection to domain SearchSection
impl From<crate::backends::api::SearchSection> for SearchSection {
    fn from(section: crate::backends::api::SearchSection) -> Self {
        SearchSection {
            key: section.key,
            title: section.title,
            items: section.items.into_iter().map(SearchItem::from).collect(),
        }
    }
}

/// Convert api::SearchResults to domain SearchResults
impl From<crate::backends::api::SearchResults> for SearchResults {
    fn from(results: crate::backends::api::SearchResults) -> Self {
        SearchResults {
            sections: results.sections.into_iter().map(SearchSection::from).collect(),
        }
    }
}
