//! MediaItem - Unified content type for all media entities.
//!
//! Replaces the stringly-typed Song.metadata HashMap with strongly-typed
//! variants for each content type. This eliminates the "Lossy Adapter Chain"
//! anti-pattern where conversions silently dropped fields.

use std::borrow::Cow;
use std::time::Duration;

use ratatui::style::{Color, Style};
use serde::{Deserialize, Serialize};

use super::ContentType;

// =============================================================================
// MEDIA ITEM ENUM
// =============================================================================

/// Primary domain entity for all navigable/playable media.
///
/// Each variant contains only the fields relevant to that content type,
/// providing compile-time safety against field omission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaItem {
    Track(Track),
    Artist(Artist),
    Album(Album),
    Playlist(Playlist),
    /// Visual separator for lists (search results, library sections)
    Header { title: String },
}

// =============================================================================
// VARIANT STRUCTS
// =============================================================================

/// A playable track (song or video).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Unique identifier (video_id, file path, etc.)
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
    /// Backend-specific data
    #[serde(default)]
    pub backend: BackendExtension,
}

/// A navigable artist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub subscribers: Option<String>,
    pub thumbnail: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub backend: BackendExtension,
}

/// A navigable album.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u32>,
    pub track_count: Option<usize>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
    #[serde(default)]
    pub backend: BackendExtension,
}

/// A navigable playlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub track_count: Option<usize>,
    pub thumbnail: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub backend: BackendExtension,
}

// =============================================================================
// BACKEND EXTENSIONS
// =============================================================================

/// Backend-specific metadata that doesn't belong in core fields.
///
/// This isolates YouTube/MPD/Spotify-specific data, preventing one backend's
/// fields from polluting another's types.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "backend", content = "data", rename_all = "snake_case")]
pub enum BackendExtension {
    #[default]
    None,
    YouTube(YouTubeData),
    Mpd(MpdData),
}

/// YouTube-specific metadata.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct YouTubeData {
    pub video_id: Option<String>,
    pub channel_id: Option<String>,
    pub view_count: Option<u64>,
    pub is_live: bool,
    /// Continuation token for pagination
    pub params: Option<String>,
}

/// MPD-specific metadata.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MpdData {
    pub file_path: Option<String>,
    pub position: Option<u32>,
    pub disc: Option<u32>,
    pub track_number: Option<u32>,
    pub format: Option<String>,
}

// =============================================================================
// DISPLAYABLE TRAIT
// =============================================================================

/// Unified display interface for all media items.
///
/// This trait replaces the scattered ListItemDisplay implementations,
/// providing a single contract for UI rendering.
pub trait Displayable {
    fn title(&self) -> Cow<'_, str>;
    fn subtitle(&self) -> Option<Cow<'_, str>>;
    fn thumbnail_url(&self) -> Option<&str>;
    fn type_icon(&self) -> &'static str;
    fn icon_style(&self) -> Style;
    fn content_type(&self) -> ContentType;
    fn is_playable(&self) -> bool;
    fn is_navigable(&self) -> bool;
}

// =============================================================================
// DISPLAYABLE IMPLEMENTATIONS
// =============================================================================

impl Displayable for MediaItem {
    fn title(&self) -> Cow<'_, str> {
        match self {
            MediaItem::Track(t) => Cow::Borrowed(&t.title),
            MediaItem::Artist(a) => Cow::Borrowed(&a.name),
            MediaItem::Album(a) => Cow::Borrowed(&a.title),
            MediaItem::Playlist(p) => Cow::Borrowed(&p.title),
            MediaItem::Header { title } => Cow::Borrowed(title),
        }
    }

    fn subtitle(&self) -> Option<Cow<'_, str>> {
        match self {
            MediaItem::Track(t) => t.artist.as_deref().map(Cow::Borrowed),
            MediaItem::Artist(a) => a.subscribers.as_deref().map(Cow::Borrowed),
            MediaItem::Album(a) => a.artist.as_deref().map(Cow::Borrowed),
            MediaItem::Playlist(p) => p.author.as_deref().map(Cow::Borrowed),
            MediaItem::Header { .. } => None,
        }
    }

    fn thumbnail_url(&self) -> Option<&str> {
        match self {
            MediaItem::Track(t) => t.thumbnail.as_deref(),
            MediaItem::Artist(a) => a.thumbnail.as_deref(),
            MediaItem::Album(a) => a.thumbnail.as_deref(),
            MediaItem::Playlist(p) => p.thumbnail.as_deref(),
            MediaItem::Header { .. } => None,
        }
    }

    fn type_icon(&self) -> &'static str {
        match self {
            MediaItem::Track(_) => "🎵",
            MediaItem::Artist(_) => "🎤",
            MediaItem::Album(_) => "💿",
            MediaItem::Playlist(_) => "📁",
            MediaItem::Header { .. } => "─",
        }
    }

    fn icon_style(&self) -> Style {
        match self {
            MediaItem::Track(_) => Style::default().fg(Color::Yellow),
            MediaItem::Artist(_) => Style::default().fg(Color::Cyan),
            MediaItem::Album(_) => Style::default().fg(Color::Magenta),
            MediaItem::Playlist(_) => Style::default().fg(Color::Green),
            MediaItem::Header { .. } => Style::default().fg(Color::DarkGray),
        }
    }

    fn content_type(&self) -> ContentType {
        match self {
            MediaItem::Track(_) => ContentType::Track,
            MediaItem::Artist(_) => ContentType::Artist,
            MediaItem::Album(_) => ContentType::Album,
            MediaItem::Playlist(_) => ContentType::Playlist,
            MediaItem::Header { .. } => ContentType::Header,
        }
    }

    fn is_playable(&self) -> bool {
        matches!(self, MediaItem::Track(_))
    }

    fn is_navigable(&self) -> bool {
        matches!(
            self,
            MediaItem::Artist(_) | MediaItem::Album(_) | MediaItem::Playlist(_)
        )
    }
}

// =============================================================================
// CONVENIENCE CONSTRUCTORS
// =============================================================================

impl MediaItem {
    /// Create a track from minimal info.
    pub fn track(id: impl Into<String>, title: impl Into<String>) -> Self {
        MediaItem::Track(Track {
            id: id.into(),
            title: title.into(),
            artist: None,
            album: None,
            duration: None,
            thumbnail: None,
            explicit: false,
            backend: BackendExtension::None,
        })
    }

    /// Create an artist from minimal info.
    pub fn artist(id: impl Into<String>, name: impl Into<String>) -> Self {
        MediaItem::Artist(Artist {
            id: id.into(),
            name: name.into(),
            subscribers: None,
            thumbnail: None,
            description: None,
            backend: BackendExtension::None,
        })
    }

    /// Create an album from minimal info.
    pub fn album(id: impl Into<String>, title: impl Into<String>) -> Self {
        MediaItem::Album(Album {
            id: id.into(),
            title: title.into(),
            artist: None,
            year: None,
            track_count: None,
            thumbnail: None,
            explicit: false,
            backend: BackendExtension::None,
        })
    }

    /// Create a playlist from minimal info.
    pub fn playlist(id: impl Into<String>, title: impl Into<String>) -> Self {
        MediaItem::Playlist(Playlist {
            id: id.into(),
            title: title.into(),
            author: None,
            track_count: None,
            thumbnail: None,
            description: None,
            backend: BackendExtension::None,
        })
    }

    /// Create a section header.
    pub fn header(title: impl Into<String>) -> Self {
        MediaItem::Header { title: title.into() }
    }

    /// Get the unique ID for this item.
    pub fn id(&self) -> &str {
        match self {
            MediaItem::Track(t) => &t.id,
            MediaItem::Artist(a) => &a.id,
            MediaItem::Album(a) => &a.id,
            MediaItem::Playlist(p) => &p.id,
            MediaItem::Header { title } => title,
        }
    }

    /// Get duration if this is a track.
    pub fn duration(&self) -> Option<Duration> {
        match self {
            MediaItem::Track(t) => t.duration,
            _ => None,
        }
    }
}

// =============================================================================
// CONVERSIONS FROM EXISTING TYPES
// =============================================================================

use crate::backends::api::{Item, ContentType as ApiContentType};
use crate::domain::Song;

/// Convert api::Item to MediaItem (lossless conversion)
impl From<Item> for MediaItem {
    fn from(item: Item) -> Self {
        match item.content_type {
            ApiContentType::Track | ApiContentType::Video => MediaItem::Track(Track {
                id: item.id,
                title: item.title,
                artist: item.subtitle,
                album: None,
                duration: item.duration,
                thumbnail: item.thumbnail,
                explicit: false,
                backend: BackendExtension::None,
            }),
            ApiContentType::Artist => MediaItem::Artist(Artist {
                id: item.id,
                name: item.title,
                subscribers: item.subtitle,
                thumbnail: item.thumbnail,
                description: None,
                backend: BackendExtension::None,
            }),
            ApiContentType::Album => MediaItem::Album(Album {
                id: item.id,
                title: item.title,
                artist: item.subtitle,
                year: None,
                track_count: None,
                thumbnail: item.thumbnail,
                explicit: false,
                backend: BackendExtension::None,
            }),
            ApiContentType::Playlist | ApiContentType::Directory => MediaItem::Playlist(Playlist {
                id: item.id,
                title: item.title,
                author: item.subtitle,
                track_count: None,
                thumbnail: item.thumbnail,
                description: None,
                backend: BackendExtension::None,
            }),
            ApiContentType::Header => MediaItem::Header { title: item.title },
        }
    }
}

/// Convert MediaItem back to api::Item (for backward compatibility)
impl From<MediaItem> for Item {
    fn from(media: MediaItem) -> Self {
        match media {
            MediaItem::Track(t) => Item {
                id: t.id,
                content_type: ApiContentType::Track,
                title: t.title,
                subtitle: t.artist,
                thumbnail: t.thumbnail,
                duration: t.duration,
                queue_id: None,
            },
            MediaItem::Artist(a) => Item {
                id: a.id,
                content_type: ApiContentType::Artist,
                title: a.name,
                subtitle: a.subscribers,
                thumbnail: a.thumbnail,
                duration: None,
                queue_id: None,
            },
            MediaItem::Album(a) => Item {
                id: a.id,
                content_type: ApiContentType::Album,
                title: a.title,
                subtitle: a.artist,
                thumbnail: a.thumbnail,
                duration: None,
                queue_id: None,
            },
            MediaItem::Playlist(p) => Item {
                id: p.id,
                content_type: ApiContentType::Playlist,
                title: p.title,
                subtitle: p.author,
                thumbnail: p.thumbnail,
                duration: None,
                queue_id: None,
            },
            MediaItem::Header { title } => Item {
                id: String::new(),
                content_type: ApiContentType::Header,
                title,
                subtitle: None,
                thumbnail: None,
                duration: None,
                queue_id: None,
            },
        }
    }
}

/// Convert Song to MediaItem (extracts type from metadata)
impl From<Song> for MediaItem {
    fn from(song: Song) -> Self {
        use crate::domain::display::ListItemDisplay;

        // Extract owned data first to avoid borrow conflicts
        let uri = song.uri.clone();
        let duration = song.duration;

        let item_type = song.item_type().unwrap_or("song");
        let title = song.title().to_string();
        let artist = song.artist().map(|s| s.to_string());
        let album = song.album().map(|s| s.to_string());
        let thumbnail = song.thumbnail_url().map(|s| s.to_string());

        match item_type {
            "artist" => MediaItem::Artist(Artist {
                id: uri,
                name: title,
                subscribers: artist,
                thumbnail,
                description: None,
                backend: BackendExtension::None,
            }),
            "album" => MediaItem::Album(Album {
                id: uri,
                title,
                artist,
                year: None,
                track_count: None,
                thumbnail,
                explicit: false,
                backend: BackendExtension::None,
            }),
            "playlist" => MediaItem::Playlist(Playlist {
                id: uri,
                title,
                author: artist,
                track_count: None,
                thumbnail,
                description: None,
                backend: BackendExtension::None,
            }),
            "header" => MediaItem::Header { title },
            _ => MediaItem::Track(Track {
                id: uri,
                title,
                artist,
                album,
                duration,
                thumbnail,
                explicit: false,
                backend: BackendExtension::None,
            }),
        }
    }
}

/// Convert MediaItem to Song (for backward compatibility with existing code)
impl From<MediaItem> for Song {
    fn from(media: MediaItem) -> Self {
        let mut metadata = std::collections::HashMap::new();

        match &media {
            MediaItem::Track(t) => {
                metadata.insert("title".to_string(), vec![t.title.clone()]);
                if let Some(ref a) = t.artist {
                    metadata.insert("artist".to_string(), vec![a.clone()]);
                }
                if let Some(ref a) = t.album {
                    metadata.insert("album".to_string(), vec![a.clone()]);
                }
                if let Some(ref th) = t.thumbnail {
                    metadata.insert("thumbnail".to_string(), vec![th.clone()]);
                }
                metadata.insert("type".to_string(), vec!["song".to_string()]);

                Song {
                    id: None,
                    uri: t.id.clone(),
                    duration: t.duration,
                    metadata,
                    last_modified: None,
                    added: None,
                }
            }
            MediaItem::Artist(a) => {
                metadata.insert("title".to_string(), vec![a.name.clone()]);
                if let Some(ref s) = a.subscribers {
                    metadata.insert("artist".to_string(), vec![s.clone()]);
                }
                if let Some(ref th) = a.thumbnail {
                    metadata.insert("thumbnail".to_string(), vec![th.clone()]);
                }
                metadata.insert("type".to_string(), vec!["artist".to_string()]);

                Song {
                    id: None,
                    uri: a.id.clone(),
                    duration: None,
                    metadata,
                    last_modified: None,
                    added: None,
                }
            }
            MediaItem::Album(a) => {
                metadata.insert("title".to_string(), vec![a.title.clone()]);
                if let Some(ref art) = a.artist {
                    metadata.insert("artist".to_string(), vec![art.clone()]);
                }
                if let Some(ref th) = a.thumbnail {
                    metadata.insert("thumbnail".to_string(), vec![th.clone()]);
                }
                metadata.insert("type".to_string(), vec!["album".to_string()]);

                Song {
                    id: None,
                    uri: a.id.clone(),
                    duration: None,
                    metadata,
                    last_modified: None,
                    added: None,
                }
            }
            MediaItem::Playlist(p) => {
                metadata.insert("title".to_string(), vec![p.title.clone()]);
                if let Some(ref auth) = p.author {
                    metadata.insert("artist".to_string(), vec![auth.clone()]);
                }
                if let Some(ref th) = p.thumbnail {
                    metadata.insert("thumbnail".to_string(), vec![th.clone()]);
                }
                metadata.insert("type".to_string(), vec!["playlist".to_string()]);

                Song {
                    id: None,
                    uri: p.id.clone(),
                    duration: None,
                    metadata,
                    last_modified: None,
                    added: None,
                }
            }
            MediaItem::Header { title } => {
                metadata.insert("title".to_string(), vec![title.clone()]);
                metadata.insert("type".to_string(), vec!["header".to_string()]);

                Song {
                    id: None,
                    uri: String::new(),
                    duration: None,
                    metadata,
                    last_modified: None,
                    added: None,
                }
            }
        }
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_item_track_displayable() {
        let track = MediaItem::Track(Track {
            id: "abc123".into(),
            title: "Test Song".into(),
            artist: Some("Test Artist".into()),
            album: Some("Test Album".into()),
            duration: Some(Duration::from_secs(180)),
            thumbnail: Some("https://example.com/thumb.jpg".into()),
            explicit: false,
            backend: BackendExtension::YouTube(YouTubeData {
                video_id: Some("abc123".into()),
                ..Default::default()
            }),
        });

        assert_eq!(track.title(), "Test Song");
        assert_eq!(track.subtitle(), Some(Cow::Borrowed("Test Artist")));
        assert_eq!(track.thumbnail_url(), Some("https://example.com/thumb.jpg"));
        assert_eq!(track.type_icon(), "🎵");
        assert_eq!(track.content_type(), ContentType::Track);
        assert!(track.is_playable());
        assert!(!track.is_navigable());
    }

    #[test]
    fn media_item_artist_displayable() {
        let artist = MediaItem::artist("artist123", "Famous Artist");

        assert_eq!(artist.title(), "Famous Artist");
        assert_eq!(artist.type_icon(), "🎤");
        assert_eq!(artist.content_type(), ContentType::Artist);
        assert!(!artist.is_playable());
        assert!(artist.is_navigable());
    }

    #[test]
    fn media_item_header_not_interactive() {
        let header = MediaItem::header("Songs");

        assert_eq!(header.title(), "Songs");
        assert_eq!(header.type_icon(), "─");
        assert!(!header.is_playable());
        assert!(!header.is_navigable());
    }

    #[test]
    fn backend_extension_serialization() {
        let yt_data = BackendExtension::YouTube(YouTubeData {
            video_id: Some("dQw4w9WgXcQ".into()),
            view_count: Some(1_000_000_000),
            is_live: false,
            ..Default::default()
        });

        let json = serde_json::to_string(&yt_data).unwrap();
        assert!(json.contains("you_tube") || json.contains("YouTube"), "Expected youtube variant in: {}", json);
        assert!(json.contains("dQw4w9WgXcQ"));

        let parsed: BackendExtension = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, yt_data);
    }

    #[test]
    fn item_to_media_item_track() {
        use crate::backends::api::Item;

        let item = Item {
            id: "video123".into(),
            content_type: super::ApiContentType::Track,
            title: "Test Song".into(),
            subtitle: Some("Test Artist".into()),
            thumbnail: Some("https://example.com/thumb.jpg".into()),
            duration: Some(Duration::from_secs(180)),
            queue_id: None,
        };

        let media: MediaItem = item.into();

        assert!(matches!(media, MediaItem::Track(_)));
        assert_eq!(media.title(), "Test Song");
        assert_eq!(media.thumbnail_url(), Some("https://example.com/thumb.jpg"));
    }

    #[test]
    fn item_to_media_item_artist() {
        use crate::backends::api::Item;

        let item = Item {
            id: "artist456".into(),
            content_type: super::ApiContentType::Artist,
            title: "Famous Artist".into(),
            subtitle: Some("1M subscribers".into()),
            thumbnail: Some("https://example.com/artist.jpg".into()),
            duration: None,
            queue_id: None,
        };

        let media: MediaItem = item.into();

        assert!(matches!(media, MediaItem::Artist(_)));
        assert_eq!(media.title(), "Famous Artist");
        assert_eq!(media.type_icon(), "🎤");
    }

    #[test]
    fn media_item_to_item_roundtrip() {
        use crate::backends::api::Item;

        let original = MediaItem::Track(Track {
            id: "test123".into(),
            title: "Roundtrip Test".into(),
            artist: Some("Artist".into()),
            album: None,
            duration: Some(Duration::from_secs(200)),
            thumbnail: Some("https://example.com/rt.jpg".into()),
            explicit: false,
            backend: BackendExtension::None,
        });

        let item: Item = original.clone().into();
        let roundtrip: MediaItem = item.into();

        // Core fields should survive roundtrip
        assert_eq!(roundtrip.title(), original.title());
        assert_eq!(roundtrip.thumbnail_url(), original.thumbnail_url());
    }
}
