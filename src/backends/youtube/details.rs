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

// =============================================================================
// CONVERSIONS: YouTube types -> New Content types (domain/content.rs)
// =============================================================================

use crate::domain::content;

impl From<PlaylistDetails> for content::PlaylistContent {
    fn from(yt: PlaylistDetails) -> Self {
        // Build extensions
        let mut extensions = content::Extensions::builder();
        
        // Add stats
        let mut stats = vec![content::Stat::track_count(yt.track_count)];
        if let Some(duration) = &yt.duration_text {
            stats.push(content::Stat::text(
                content::StatKey::Duration,
                "Duration",
                duration.clone()
            ));
        }
        extensions = extensions.stats(stats);
        
        // Add actions
        extensions = extensions.actions(vec![
            content::Action::play(),
            content::Action::shuffle(),
            content::Action::add_to_queue(),
        ]);
        
        // Add featured artists section
        if !yt.featured_artists.is_empty() {
            let artists: Vec<content::ContentRef> = yt.featured_artists.into_iter()
                .map(|a| content::ContentRef::artist(a.id, a.name))
                .collect();
            extensions = extensions.featured_artists("Featured artists", artists);
        }
        
        // Add related playlists section
        if !yt.related_playlists.is_empty() {
            let playlists: Vec<content::ContentRef> = yt.related_playlists.into_iter()
                .map(|p| content::ContentRef::playlist(p.id, p.title)
                    .with_subtitle(p.subtitle.unwrap_or_default()))
                .collect();
            extensions = extensions.related_playlists("Similar playlists", playlists);
        }
        
        content::PlaylistContent {
            id: yt.id,
            title: yt.title,
            tracks: yt.tracks,
            author: yt.artist.map(|name| content::ContentRef::new("", name)),
            thumbnail: yt.thumbnail,
            description: None,
            track_count: Some(yt.track_count),
            duration_text: yt.duration_text,
            extensions: extensions.build(),
        }
    }
}

impl From<AlbumDetails> for content::AlbumContent {
    fn from(yt: AlbumDetails) -> Self {
        // Build extensions
        let mut extensions = content::Extensions::builder();
        
        // Add stats
        let mut stats = vec![content::Stat::track_count(yt.tracks.len())];
        if let Some(year) = &yt.year {
            if let Ok(y) = year.parse::<u16>() {
                stats.insert(0, content::Stat::year(y));
            }
        }
        extensions = extensions.stats(stats);
        
        // Add actions
        extensions = extensions.actions(vec![
            content::Action::play(),
            content::Action::shuffle(),
            content::Action::add_to_queue(),
        ]);
        
        // Add "more by artist" section
        if !yt.more_by_artist.is_empty() {
            let more_albums: Vec<content::ContentRef> = yt.more_by_artist.into_iter()
                .map(|a| content::ContentRef::album(a.id, a.title)
                    .with_subtitle(a.year.unwrap_or_default()))
                .collect();
            extensions = extensions.more_by_artist(
                format!("More by {}", yt.artist.name),
                more_albums
            );
        }
        
        content::AlbumContent {
            id: yt.id,
            title: yt.title,
            artist: content::ContentRef::artist(yt.artist.id, yt.artist.name)
                .with_thumbnail(yt.artist.thumbnail.unwrap_or_default()),
            tracks: yt.tracks,
            thumbnail: yt.thumbnail,
            year: yt.year.and_then(|y| y.parse().ok()),
            release_type: None,
            description: None,
            extensions: extensions.build(),
        }
    }
}

impl From<ArtistDetails> for content::ArtistContent {
    fn from(yt: ArtistDetails) -> Self {
        // Build extensions
        let mut extensions = content::Extensions::builder();
        
        // Add stats
        let mut stats = vec![];
        if let Some(subs) = &yt.subscribers {
            stats.push(content::Stat::subscribers(subs.clone()));
        }
        extensions = extensions.stats(stats);
        
        // Add actions
        extensions = extensions.actions(vec![
            content::Action::play(),
            content::Action::shuffle(),
            content::Action::radio(),
        ]);
        
        // Add albums section
        if !yt.albums.is_empty() {
            let albums: Vec<content::ContentRef> = yt.albums.into_iter()
                .map(|a| content::ContentRef::album(a.id, a.title)
                    .with_subtitle(a.year.unwrap_or_default()))
                .collect();
            extensions = extensions.albums("Albums", albums);
        }
        
        // Add singles section
        if !yt.singles.is_empty() {
            let singles: Vec<content::ContentRef> = yt.singles.into_iter()
                .map(|a| content::ContentRef::album(a.id, a.title)
                    .with_subtitle(a.year.unwrap_or_default()))
                .collect();
            extensions = extensions.singles("Singles", singles);
        }
        
        // Add related artists section
        if !yt.related_artists.is_empty() {
            let related: Vec<content::ContentRef> = yt.related_artists.into_iter()
                .map(|a| content::ContentRef::artist(a.id, a.name))
                .collect();
            extensions = extensions.related_artists("Fans also like", related);
        }
        
        content::ArtistContent {
            id: yt.id,
            name: yt.name,
            top_songs: yt.top_songs,
            thumbnail: yt.thumbnail,
            bio: yt.description,
            extensions: extensions.build(),
        }
    }
}
