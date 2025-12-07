//! Item struct definitions for search results

use std::time::Duration;
use std::collections::HashMap;
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
            file: item.video_id.clone(),
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
            file: item.video_id.clone(),
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
        Self {
            browse_id: None,
            name: String::new(),
            subscribers: None,
            thumbnail: None,
        }
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
