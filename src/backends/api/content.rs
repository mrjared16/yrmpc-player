//! Content types and capabilities.
//!
//! Defines the universal content model used across all backends.

use std::time::Duration;

/// Type of content item
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ContentType {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
    Directory,
    Header, // Section header in search results
}

/// A displayable/playable item
///
/// Universal type for tracks, albums, playlists in lists.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// Unique identifier (video ID, file path, playlist ID)
    pub id: String,
    /// What kind of content
    pub content_type: ContentType,
    /// Primary text (song title, album name)
    pub title: String,
    /// Secondary text (artist, owner)
    pub subtitle: Option<String>,
    /// Thumbnail URL
    pub thumbnail: Option<String>,
    /// Duration (for tracks)
    pub duration: Option<Duration>,
    /// Queue position ID (only when in queue)
    pub queue_id: Option<u32>,
}

impl Item {
    pub fn track(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            content_type: ContentType::Track,
            ..Default::default()
        }
    }

    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.subtitle = Some(artist.into());
        self
    }

    pub fn with_thumbnail(mut self, url: impl Into<String>) -> Self {
        self.thumbnail = Some(url.into());
        self
    }

    pub fn with_duration(mut self, d: Duration) -> Self {
        self.duration = Some(d);
        self
    }

    /// Check if this item is directly playable (track or video)
    pub fn is_playable(&self) -> bool {
        matches!(self.content_type, ContentType::Track)
    }

    /// Check if this item needs resolution (album, playlist, artist)
    pub fn needs_resolve(&self) -> bool {
        matches!(
            self.content_type,
            ContentType::Album | ContentType::Playlist | ContentType::Artist
        )
    }
}

/// Backend capabilities (for TUI to show/hide features)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Can browse/edit saved playlists
    SavedPlaylists,
    /// Rich metadata (thumbnails, full artist info)
    RichMetadata,
    /// MPD stickers (rating, tags)
    Stickers,
    /// MPD audio outputs
    Outputs,
}

// === Conversions ===

impl From<&crate::domain::Song> for Item {
    fn from(song: &crate::domain::Song) -> Self {
        let title = song.metadata.get("title")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_else(|| song.uri.clone());

        let subtitle = song.metadata.get("artist")
            .and_then(|v| v.first())
            .cloned();

        let thumbnail = song.metadata.get("thumbnail")
            .and_then(|v| v.first())
            .cloned();

        let content_type = song.metadata.get("type")
            .and_then(|v| v.first())
            .map(|t| match t.as_str() {
                "album" => ContentType::Album,
                "artist" => ContentType::Artist,
                "playlist" => ContentType::Playlist,
                "header" => ContentType::Header,
                _ => ContentType::Track,
            })
            .unwrap_or(ContentType::Track);

        Item {
            id: song.uri.clone(),
            content_type,
            title,
            subtitle,
            thumbnail,
            duration: song.duration,
            queue_id: song.id,
        }
    }
}
