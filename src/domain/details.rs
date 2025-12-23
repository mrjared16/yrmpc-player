//! Content detail types for albums, artists, and playlists.
//!
//! These are backend-agnostic domain types representing detailed views
//! of content. Backends fetch data and populate these types; UI renders them.
//!
//! # Design Notes
//!
//! - All fields that may not be available are `Option<T>`
//! - Uses `Song` from the same domain module for tracks
//! - Reference types (`ArtistRef`, etc.) are for navigation links
//! - Detail types contain the full content with tracks/related items

use super::Song;

// =============================================================================
// REFERENCE TYPES (for navigation links)
// =============================================================================

/// Reference to an artist for navigation.
///
/// Used when displaying artist links that can be clicked to view full details.
#[derive(Debug, Clone, Default)]
pub struct ArtistRef {
    /// Artist identifier (channel ID for YouTube, artist path for MPD)
    pub id: String,
    /// Display name
    pub name: String,
    /// Profile image URL
    pub thumbnail: Option<String>,
}

impl ArtistRef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            thumbnail: None,
        }
    }

    pub fn with_thumbnail(mut self, url: impl Into<String>) -> Self {
        self.thumbnail = Some(url.into());
        self
    }
}

/// Reference to an album for navigation.
///
/// Used when displaying album links in artist pages, "more by artist", etc.
#[derive(Debug, Clone, Default)]
pub struct AlbumRef {
    /// Album identifier
    pub id: String,
    /// Album title
    pub title: String,
    /// Release year
    pub year: Option<String>,
    /// Cover art URL
    pub thumbnail: Option<String>,
}

impl AlbumRef {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            year: None,
            thumbnail: None,
        }
    }
}

/// Reference to a playlist for navigation.
///
/// Used when displaying playlist links in related content sections.
#[derive(Debug, Clone, Default)]
pub struct PlaylistRef {
    /// Playlist identifier
    pub id: String,
    /// Playlist title
    pub title: String,
    /// Secondary text (creator name, track count, etc.)
    pub subtitle: Option<String>,
    /// Cover art URL
    pub thumbnail: Option<String>,
}

impl PlaylistRef {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            thumbnail: None,
        }
    }
}

// =============================================================================
// DETAIL TYPES (full content views)
// =============================================================================

/// Full album details with tracks and related content.
///
/// Returned by `Discovery::details()` for album items.
#[derive(Debug, Clone, Default)]
pub struct AlbumDetails {
    /// Album identifier
    pub id: String,
    /// Album title
    pub title: String,
    /// Primary artist (simplified - could be extended to Vec for compilations)
    pub artist: ArtistRef,
    /// Release year
    pub year: Option<String>,
    /// Album description or notes
    pub description: Option<String>,
    /// Cover art URL
    pub thumbnail: Option<String>,
    /// Album tracks in order
    pub tracks: Vec<Song>,
    /// Other albums by the same artist
    pub more_by_artist: Vec<AlbumRef>,
}

impl AlbumDetails {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }
}

/// Full artist details with biography, discography, and related content.
///
/// Returned by `Discovery::details()` for artist items.
#[derive(Debug, Clone, Default)]
pub struct ArtistDetails {
    /// Artist identifier
    pub id: String,
    /// Artist name
    pub name: String,
    /// Subscriber/follower count (platform-specific, may be None)
    pub subscribers: Option<String>,
    /// Artist biography or description
    pub description: Option<String>,
    /// Profile image URL
    pub thumbnail: Option<String>,
    /// Top/popular songs
    pub top_songs: Vec<Song>,
    /// Full albums
    pub albums: Vec<AlbumRef>,
    /// Singles and EPs
    pub singles: Vec<AlbumRef>,
    /// Similar/related artists
    pub related_artists: Vec<ArtistRef>,
}

impl ArtistDetails {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }
}

/// Full playlist details with tracks and related content.
///
/// Returned by `Discovery::details()` for playlist items.
#[derive(Debug, Clone, Default)]
pub struct PlaylistDetails {
    /// Playlist identifier
    pub id: String,
    /// Playlist title
    pub title: String,
    /// Creator/author name
    pub author: Option<String>,
    /// Last updated year or creation year
    pub year: Option<String>,
    /// Playlist description
    pub description: Option<String>,
    /// Cover art URL
    pub thumbnail: Option<String>,
    /// Total number of tracks
    pub track_count: usize,
    /// Total duration as display text (e.g., "2 hr 45 min")
    pub duration_text: Option<String>,
    /// Playlist tracks in order
    pub tracks: Vec<Song>,
    /// Featured artists in this playlist
    pub featured_artists: Vec<ArtistRef>,
    /// Similar/related playlists
    pub related_playlists: Vec<PlaylistRef>,
}

impl PlaylistDetails {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }
}

// =============================================================================
// CONTENT DETAILS ENUM
// =============================================================================

/// Unified return type for `Discovery::details()`.
///
/// Wraps the type-specific detail structs in an enum for dispatch.
/// UI code can pattern match to get the specific type.
///
/// # Example
///
/// ```ignore
/// match backend.details(&item)? {
///     ContentDetails::Album(album) => show_album_page(album),
///     ContentDetails::Artist(artist) => show_artist_page(artist),
///     ContentDetails::Playlist(playlist) => show_playlist_page(playlist),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum ContentDetails {
    Album(AlbumDetails),
    Artist(ArtistDetails),
    Playlist(PlaylistDetails),
}

impl ContentDetails {
    /// Get the content ID regardless of type.
    pub fn id(&self) -> &str {
        match self {
            ContentDetails::Album(a) => &a.id,
            ContentDetails::Artist(a) => &a.id,
            ContentDetails::Playlist(p) => &p.id,
        }
    }

    /// Get the title regardless of type.
    pub fn title(&self) -> &str {
        match self {
            ContentDetails::Album(a) => &a.title,
            ContentDetails::Artist(a) => &a.name,
            ContentDetails::Playlist(p) => &p.title,
        }
    }

    /// Get the thumbnail URL if available.
    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            ContentDetails::Album(a) => a.thumbnail.as_deref(),
            ContentDetails::Artist(a) => a.thumbnail.as_deref(),
            ContentDetails::Playlist(p) => p.thumbnail.as_deref(),
        }
    }

    /// Check if this is an album.
    pub fn is_album(&self) -> bool {
        matches!(self, ContentDetails::Album(_))
    }

    /// Check if this is an artist.
    pub fn is_artist(&self) -> bool {
        matches!(self, ContentDetails::Artist(_))
    }

    /// Check if this is a playlist.
    pub fn is_playlist(&self) -> bool {
        matches!(self, ContentDetails::Playlist(_))
    }

    /// Try to get as album details.
    pub fn as_album(&self) -> Option<&AlbumDetails> {
        match self {
            ContentDetails::Album(a) => Some(a),
            _ => None,
        }
    }

    /// Try to get as artist details.
    pub fn as_artist(&self) -> Option<&ArtistDetails> {
        match self {
            ContentDetails::Artist(a) => Some(a),
            _ => None,
        }
    }

    /// Try to get as playlist details.
    pub fn as_playlist(&self) -> Option<&PlaylistDetails> {
        match self {
            ContentDetails::Playlist(p) => Some(p),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artist_ref_builder() {
        let artist = ArtistRef::new("id123", "The Beatles")
            .with_thumbnail("https://example.com/beatles.jpg");
        
        assert_eq!(artist.id, "id123");
        assert_eq!(artist.name, "The Beatles");
        assert!(artist.thumbnail.is_some());
    }

    #[test]
    fn test_album_details_default() {
        let album = AlbumDetails::new("album1", "Abbey Road");
        
        assert_eq!(album.id, "album1");
        assert_eq!(album.title, "Abbey Road");
        assert!(album.tracks.is_empty());
        assert!(album.description.is_none());
    }

    #[test]
    fn test_content_details_helpers() {
        let album = ContentDetails::Album(AlbumDetails {
            id: "a1".into(),
            title: "Test Album".into(),
            thumbnail: Some("https://example.com/cover.jpg".into()),
            ..Default::default()
        });

        assert!(album.is_album());
        assert!(!album.is_artist());
        assert_eq!(album.id(), "a1");
        assert_eq!(album.title(), "Test Album");
        assert!(album.as_album().is_some());
        assert!(album.as_artist().is_none());
    }
}
