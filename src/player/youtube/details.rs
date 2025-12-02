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
