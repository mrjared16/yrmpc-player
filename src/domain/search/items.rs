//! Item struct definitions for search results

use std::{collections::HashMap, time::Duration};

use crate::domain::Song;

/// A song from search results
#[derive(Debug, Clone, PartialEq)]
pub struct SongItem {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
    /// Radio playlist ID - when present, this song is from a radio/station
    /// search and can be used to auto-populate the queue with related
    /// tracks
    pub radio_playlist_id: Option<String>,
}

/// A video from search results
#[derive(Debug, Clone, PartialEq)]
pub struct VideoItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub views: Option<String>,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
}

/// An artist from search results
#[derive(Debug, Clone, PartialEq)]
pub struct ArtistItem {
    /// Can be None for some TopResult artists
    pub browse_id: Option<String>,
    pub name: String,
    pub subscribers: Option<String>,
    pub thumbnail: Option<String>,
}

/// An album from search results
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumItem {
    pub album_id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    pub album_type: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

/// A playlist from search results
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItem {
    pub playlist_id: String,
    pub title: String,
    pub author: String,
    pub track_count: Option<String>,
    pub thumbnail: Option<String>,
}

// ============ Conversions to domain::Song for queue ============

impl From<&SongItem> for Song {
    fn from(item: &SongItem) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), vec![item.title.clone()]);
        metadata.insert("artist".to_string(), vec![item.artist.clone()]);
        metadata.insert("type".to_string(), vec!["song".to_string()]);

        if let Some(ref album) = item.album {
            metadata.insert("album".to_string(), vec![album.clone()]);
        }
        if let Some(ref thumb) = item.thumbnail {
            metadata.insert("thumbnail".to_string(), vec![thumb.clone()]);
        }

        Song {
            id: None,
            uri: item.video_id.clone(),
            duration: item.duration,
            metadata,
            last_modified: None,
            added: None,
        }
    }
}

impl From<&VideoItem> for Song {
    fn from(item: &VideoItem) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), vec![item.title.clone()]);
        metadata.insert("artist".to_string(), vec![item.channel.clone()]);
        metadata.insert("type".to_string(), vec!["video".to_string()]);

        if let Some(ref thumb) = item.thumbnail {
            metadata.insert("thumbnail".to_string(), vec![thumb.clone()]);
        }

        Song {
            id: None,
            uri: item.video_id.clone(),
            duration: item.duration,
            metadata,
            last_modified: None,
            added: None,
        }
    }
}

// ============ Default implementations ============

impl Default for SongItem {
    fn default() -> Self {
        Self {
            video_id: String::new(),
            title: String::new(),
            artist: String::new(),
            album: None,
            duration: None,
            thumbnail: None,
            explicit: false,
            radio_playlist_id: None,
        }
    }
}

impl Default for VideoItem {
    fn default() -> Self {
        Self {
            video_id: String::new(),
            title: String::new(),
            channel: String::new(),
            views: None,
            duration: None,
            thumbnail: None,
        }
    }
}

impl Default for ArtistItem {
    fn default() -> Self {
        Self { browse_id: None, name: String::new(), subscribers: None, thumbnail: None }
    }
}

impl Default for AlbumItem {
    fn default() -> Self {
        Self {
            album_id: String::new(),
            title: String::new(),
            artist: String::new(),
            year: None,
            album_type: None,
            thumbnail: None,
            explicit: false,
        }
    }
}

impl Default for PlaylistItem {
    fn default() -> Self {
        Self {
            playlist_id: String::new(),
            title: String::new(),
            author: String::new(),
            track_count: None,
            thumbnail: None,
        }
    }
}

// ============ ContentUri methods ============
// These construct ContentUri on-the-fly from existing ID fields
// for backwards compatibility during migration.

use super::{BrowsableItem, PlayableItem, SearchItem};
use crate::domain::content_uri::ContentUri;

impl SongItem {
    /// Get the ContentUri for this song
    pub fn content_uri(&self) -> ContentUri {
        ContentUri::youtube_video(&self.video_id)
    }
}

impl VideoItem {
    /// Get the ContentUri for this video
    pub fn content_uri(&self) -> ContentUri {
        ContentUri::youtube_video(&self.video_id)
    }
}

impl ArtistItem {
    /// Get the ContentUri for this artist
    pub fn content_uri(&self) -> Option<ContentUri> {
        self.browse_id.as_ref().map(ContentUri::youtube_artist)
    }
}

impl AlbumItem {
    /// Get the ContentUri for this album
    pub fn content_uri(&self) -> ContentUri {
        ContentUri::youtube_album(&self.album_id)
    }
}

impl PlaylistItem {
    /// Get the ContentUri for this playlist
    pub fn content_uri(&self) -> ContentUri {
        ContentUri::youtube_playlist(&self.playlist_id)
    }
}

impl PlayableItem {
    /// Get the ContentUri for this playable item
    pub fn content_uri(&self) -> ContentUri {
        match self {
            Self::Song(s) => s.content_uri(),
            Self::Video(v) => v.content_uri(),
        }
    }
}

impl BrowsableItem {
    /// Get the ContentUri for this browsable item
    pub fn content_uri(&self) -> Option<ContentUri> {
        match self {
            Self::Artist(a) => a.content_uri(),
            Self::Album(a) => Some(a.content_uri()),
            Self::Playlist(p) => Some(p.content_uri()),
        }
    }
}

impl SearchItem {
    /// Get the ContentUri for this search item
    pub fn content_uri(&self) -> Option<ContentUri> {
        match self {
            Self::Playable(p) => Some(p.content_uri()),
            Self::Browsable(b) => b.content_uri(),
        }
    }
}
