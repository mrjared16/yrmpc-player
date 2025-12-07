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
#[derive(Debug, Clone, PartialEq)]
pub enum SearchItem {
    /// Songs and videos - can be queued directly
    Playable(PlayableItem),
    /// Artists, albums, playlists - open detail view
    Browsable(BrowsableItem),
    /// UI section separator
    Header(String),
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
    /// No action (headers)
    None,
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

// ============ Implementations ============

impl SearchItem {
    /// Get the action for Enter key
    pub fn action(&self) -> ItemAction {
        match self {
            Self::Playable(p) => ItemAction::Play(p.video_id().to_string()),
            Self::Browsable(b) => ItemAction::Browse(b.browse_path()),
            Self::Header(_) => ItemAction::None,
        }
    }

    /// Check if this item is a section header
    pub fn is_header(&self) -> bool {
        matches!(self, Self::Header(_))
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
