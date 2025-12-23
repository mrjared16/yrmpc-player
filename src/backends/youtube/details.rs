use crate::domain::Song;

/// Reference to an artist for navigation
#[derive(Debug, Clone)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
}

/// Reference to a playlist for navigation
#[derive(Debug, Clone)]
pub struct PlaylistRef {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub subtitle: Option<String>,
}

/// Reference to an album for navigation
#[derive(Debug, Clone)]
pub struct AlbumRef {
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
}

/// Details for a playlist view
#[derive(Debug, Clone)]
pub struct PlaylistDetails {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
    pub track_count: usize,
    pub duration_text: Option<String>,
    pub tracks: Vec<Song>,
    pub featured_artists: Vec<ArtistRef>,
    pub related_playlists: Vec<PlaylistRef>,
}

/// Details for an album view
#[derive(Debug, Clone)]
pub struct AlbumDetails {
    pub id: String,
    pub title: String,
    pub artist: ArtistRef,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
    pub tracks: Vec<Song>,
    pub more_by_artist: Vec<AlbumRef>,
}

/// Details for an artist view
#[derive(Debug, Clone)]
pub struct ArtistDetails {
    pub id: String,
    pub name: String,
    pub subscribers: Option<String>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    pub top_songs: Vec<Song>,
    pub albums: Vec<AlbumRef>,
    pub singles: Vec<AlbumRef>,
    pub related_artists: Vec<ArtistRef>,
}

// =============================================================================
// CONVERSIONS: YouTube types -> Domain types
// =============================================================================

use crate::domain::details as domain;

impl From<ArtistRef> for domain::ArtistRef {
    fn from(yt: ArtistRef) -> Self {
        domain::ArtistRef {
            id: yt.id,
            name: yt.name,
            thumbnail: yt.thumbnail,
        }
    }
}

impl From<AlbumRef> for domain::AlbumRef {
    fn from(yt: AlbumRef) -> Self {
        domain::AlbumRef {
            id: yt.id,
            title: yt.title,
            year: yt.year,
            thumbnail: yt.thumbnail,
        }
    }
}

impl From<PlaylistRef> for domain::PlaylistRef {
    fn from(yt: PlaylistRef) -> Self {
        domain::PlaylistRef {
            id: yt.id,
            title: yt.title,
            subtitle: yt.subtitle,
            thumbnail: yt.thumbnail,
        }
    }
}

impl From<PlaylistDetails> for domain::PlaylistDetails {
    fn from(yt: PlaylistDetails) -> Self {
        domain::PlaylistDetails {
            id: yt.id,
            title: yt.title,
            author: yt.artist,
            year: yt.year,
            description: None, // YouTube type doesn't have description
            thumbnail: yt.thumbnail,
            track_count: yt.track_count,
            duration_text: yt.duration_text,
            tracks: yt.tracks,
            featured_artists: yt.featured_artists.into_iter().map(Into::into).collect(),
            related_playlists: yt.related_playlists.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<AlbumDetails> for domain::AlbumDetails {
    fn from(yt: AlbumDetails) -> Self {
        domain::AlbumDetails {
            id: yt.id,
            title: yt.title,
            artist: yt.artist.into(),
            year: yt.year,
            description: None, // YouTube type doesn't have description
            thumbnail: yt.thumbnail,
            tracks: yt.tracks,
            more_by_artist: yt.more_by_artist.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ArtistDetails> for domain::ArtistDetails {
    fn from(yt: ArtistDetails) -> Self {
        domain::ArtistDetails {
            id: yt.id,
            name: yt.name,
            subscribers: yt.subscribers,
            description: yt.description,
            thumbnail: yt.thumbnail,
            top_songs: yt.top_songs,
            albums: yt.albums.into_iter().map(Into::into).collect(),
            singles: yt.singles.into_iter().map(Into::into).collect(),
            related_artists: yt.related_artists.into_iter().map(Into::into).collect(),
        }
    }
}
