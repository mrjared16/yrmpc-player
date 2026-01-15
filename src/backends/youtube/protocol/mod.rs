//! IPC protocol for YouTube backend server-client communication.
//! This allows testing client and server independently.

pub mod play_intent;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::domain::{MediaItem, PlaybackState, Song, Status, status::OnOffOneshot};
use play_intent::{PlayIntent, RequestId, PlayError};

/// Commands sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerCommand {
    // Playback control
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    SeekAbsolute(f64),
    SeekRelative(f64),
    PlayPos(usize),
    PlayId(u32),
    
    // Intent-based playback (new architecture)
    /// Start playback with explicit intent.
    PlayWithIntent {
        intent: PlayIntent,
        request_id: RequestId,
    },
    
    /// Cancel pending preparation work for a request.
    CancelRequest {
        request_id: RequestId,
    },

    // Queue management
    Add {
        uri: String,
        position: Option<u32>,
    },
    /// Add song with full metadata (preferred over Add for preserving metadata)
    AddSong {
        song: SongData,
        position: Option<u32>,
    },
    DeleteId(u32),
    Clear,
    MoveId {
        from: u32,
        to: u32,
    },

    // Volume
    GetVolume,
    SetVolume(u8),
    AdjustVolume(i8),

    // Playback options
    /// Set repeat mode: "off", "one", "all"
    SetRepeat(String),
    /// Set shuffle mode: true/false
    SetShuffle(bool),

    // Status/info
    GetStatus,
    GetCurrentSong,
    GetPlaylist,

    // Search/browse
    Search {
        query: String,
    },
    Browse {
        path: String,
    },
    /// Get search suggestions for autocomplete
    GetSearchSuggestions {
        query: String,
    },

    // Rich browse details (YouTube-specific)
    /// Get detailed playlist info with tracks and metadata
    BrowsePlaylistDetails {
        playlist_id: String,
    },
    /// Get detailed album info with tracks and metadata
    BrowseAlbumDetails {
        album_id: String,
    },
    /// Get detailed artist info with discography
    BrowseArtistDetails {
        artist_id: String,
    },

    // Library
    GetLibrary {
        category: String,
    },

    // Lifecycle
    Ping,
    Shutdown,

    /// Subscribe to events (like MPD idle)
    /// Client blocks until server sends events
    Idle {
        subsystems: Vec<String>,
    },
}

/// Responses from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerResponse {
    Ok,
    Error(String),
    Status(StatusData),
    Song(Option<SongData>),
    Playlist(Vec<SongData>),
    /// Type-safe search results using MediaItem directly (no intermediate
    /// types)
    SearchResults(Vec<MediaItem>),
    BrowseResults(Vec<BrowseEntry>),
    Library(Vec<BrowseEntry>),
    /// Search suggestions for autocomplete
    Suggestions(Vec<String>),
    Volume(u8),
    Pong,

    /// Idle events - sent when subscribed subsystems change
    /// Subsystems: "playlist", "player", "mixer", "options"
    IdleEvents(Vec<String>),

    // Rich browse details responses
    /// Detailed playlist info
    PlaylistDetails(PlaylistDetailsData),
    /// Detailed album info
    AlbumDetails(AlbumDetailsData),
    /// Detailed artist info
    ArtistDetails(ArtistDetailsData),
    
    PlayResult(Result<(), PlayError>),
}

/// Serializable status data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusData {
    pub state: String, // "play", "pause", "stop"
    pub volume: u8,
    pub elapsed_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub playlist_length: u32,
    pub current_pos: Option<u32>,
    pub current_id: Option<u32>,
    /// Repeat mode: "off", "one", "all"
    pub repeat: String,
    /// Shuffle enabled
    pub shuffle: bool,
    /// Next queue position (for shuffle mode UI indicator)
    #[serde(default)]
    pub next_queue_pos: Option<u32>,
}

impl From<Status> for StatusData {
    fn from(s: Status) -> Self {
        Self {
            state: match s.state {
                PlaybackState::Play => "play".into(),
                PlaybackState::Pause => "pause".into(),
                PlaybackState::Stop => "stop".into(),
            },
            volume: s.volume,
            elapsed_ms: s.elapsed.map(|d| d.as_millis() as u64),
            duration_ms: s.duration.map(|d| d.as_millis() as u64),
            playlist_length: s.playlistlength,
            current_pos: s.song_position,
            current_id: s.songid,
            repeat: "off".into(),
            shuffle: false,
            next_queue_pos: None,
        }
    }
}

impl StatusData {
    pub fn to_status(&self) -> Status {
        Status {
            state: match self.state.as_str() {
                "play" => PlaybackState::Play,
                "pause" => PlaybackState::Pause,
                _ => PlaybackState::Stop,
            },
            volume: self.volume,
            elapsed: self.elapsed_ms.map(Duration::from_millis),
            duration: self.duration_ms.map(Duration::from_millis),
            playlistlength: self.playlist_length,
            song_position: self.current_pos,
            next_song_position: self.next_queue_pos,
            songid: self.current_id,
            repeat: self.repeat != "off",
            single: if self.repeat == "one" { OnOffOneshot::On } else { OnOffOneshot::Off },
            random: self.shuffle,
            ..Default::default()
        }
    }
}

/// Serializable song data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongData {
    pub id: Option<u32>,
    pub file: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnail: Option<String>,
    pub item_type: Option<String>, // "song", "video", "album", "artist", "playlist", "header"
}

impl From<Song> for SongData {
    fn from(s: Song) -> Self {
        Self {
            id: s.id,
            file: s.uri, // Domain Song uses 'uri', protocol uses 'file'
            title: s.metadata.get("title").and_then(|v| v.first()).cloned(),
            artist: s.metadata.get("artist").and_then(|v| v.first()).cloned(),
            album: s.metadata.get("album").and_then(|v| v.first()).cloned(),
            duration_ms: s.duration.map(|d| d.as_millis() as u64),
            thumbnail: s.metadata.get("thumbnail").and_then(|v| v.first()).cloned(),
            item_type: s.metadata.get("type").and_then(|v| v.first()).cloned(),
        }
    }
}

impl SongData {
    pub fn to_song(&self) -> Song {
        let mut metadata = std::collections::HashMap::new();
        if let Some(ref t) = self.title {
            metadata.insert("title".into(), vec![t.clone()]);
        }
        if let Some(ref a) = self.artist {
            metadata.insert("artist".into(), vec![a.clone()]);
        }
        if let Some(ref a) = self.album {
            metadata.insert("album".into(), vec![a.clone()]);
        }
        if let Some(ref t) = self.thumbnail {
            metadata.insert("thumbnail".into(), vec![t.clone()]);
        }
        if let Some(ref t) = self.item_type {
            metadata.insert("type".into(), vec![t.clone()]);
        }
        Song {
            id: self.id,
            uri: self.file.clone(), // SongData uses 'file', domain Song uses 'uri'
            duration: self.duration_ms.map(Duration::from_millis),
            metadata,
            ..Default::default()
        }
    }
}

/// Serializable search item data with type safety
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchItemData {
    Song(PlayableData),
    Video(PlayableData),
    Artist(BrowsableData),
    Album(BrowsableData),
    Playlist(BrowsableData),
    /// Section header for grouping search results (e.g., "Top Results",
    /// "Songs")
    Header {
        title: String,
    },
}

/// Data for playable items (songs/videos)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayableData {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnail: Option<String>,
}

/// Data for browsable items (artists/albums/playlists)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowsableData {
    pub browse_id: Option<String>,
    pub browse_path: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub can_queue: bool,
}

impl From<crate::domain::search::SearchItem> for SearchItemData {
    fn from(item: crate::domain::search::SearchItem) -> Self {
        use crate::domain::search::{BrowsableItem, Displayable, PlayableItem, SearchItem};

        match item {
            SearchItem::Playable(PlayableItem::Song(s)) => SearchItemData::Song(PlayableData {
                video_id: s.video_id,
                title: s.title,
                artist: s.artist,
                album: s.album,
                duration_ms: s.duration.map(|d| d.as_millis() as u64),
                thumbnail: s.thumbnail,
            }),
            SearchItem::Playable(PlayableItem::Video(v)) => SearchItemData::Video(PlayableData {
                video_id: v.video_id,
                title: v.title,
                artist: v.channel,
                album: None,
                duration_ms: v.duration.map(|d| d.as_millis() as u64),
                thumbnail: v.thumbnail,
            }),
            SearchItem::Browsable(BrowsableItem::Artist(a)) => {
                SearchItemData::Artist(BrowsableData {
                    browse_id: a.browse_id.clone(),
                    browse_path: format!("artist:{}", a.browse_id.as_ref().unwrap_or(&a.name)),
                    title: a.name,
                    subtitle: a.subscribers,
                    thumbnail: a.thumbnail,
                    can_queue: false,
                })
            }
            SearchItem::Browsable(BrowsableItem::Album(a)) => {
                SearchItemData::Album(BrowsableData {
                    browse_id: Some(a.album_id.clone()),
                    browse_path: format!("album:{}", a.album_id),
                    title: a.title,
                    subtitle: Some(format!(
                        "{}{}",
                        a.artist,
                        a.year.as_ref().map(|y| format!(" · {}", y)).unwrap_or_default()
                    )),
                    thumbnail: a.thumbnail,
                    can_queue: true,
                })
            }
            SearchItem::Browsable(BrowsableItem::Playlist(p)) => {
                SearchItemData::Playlist(BrowsableData {
                    browse_id: Some(p.playlist_id.clone()),
                    browse_path: format!("playlist:{}", p.playlist_id),
                    title: p.title,
                    subtitle: Some(format!(
                        "{}{}",
                        p.author,
                        p.track_count
                            .as_ref()
                            .map(|c| format!(" · {} tracks", c))
                            .unwrap_or_default()
                    )),
                    thumbnail: p.thumbnail,
                    can_queue: true,
                })
            }
        }
    }
}

impl SearchItemData {
    /// Convert to SongData for backward compatibility with queue operations
    pub fn to_song_data(&self) -> Option<SongData> {
        match self {
            SearchItemData::Song(p) | SearchItemData::Video(p) => Some(SongData {
                id: None,
                file: p.video_id.clone(),
                title: Some(p.title.clone()),
                artist: Some(p.artist.clone()),
                album: p.album.clone(),
                duration_ms: p.duration_ms,
                thumbnail: p.thumbnail.clone(),
                item_type: Some(
                    if matches!(self, SearchItemData::Song(_)) { "song" } else { "video" }.into(),
                ),
            }),
            _ => None,
        }
    }
}

/// Browse entry (directory or file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrowseEntry {
    Dir { name: String, path: String },
    File(SongData),
}

// ============================================================================
// Rich Browse Details Data Structures
// ============================================================================

/// Reference to an artist for navigation (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistRefData {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
}

/// Reference to an album for navigation (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumRefData {
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
}

/// Reference to a playlist for navigation (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistRefData {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub subtitle: Option<String>,
}

/// Detailed playlist info (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistDetailsData {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
    pub track_count: usize,
    pub duration_text: Option<String>,
    /// Tracks as MediaItem (canonical type, no conversion needed)
    pub tracks: Vec<MediaItem>,
    pub featured_artists: Vec<MediaItem>,
    pub related_playlists: Vec<MediaItem>,
}

/// Detailed album info (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumDetailsData {
    pub id: String,
    pub title: String,
    /// Primary artist as MediaItem
    pub artist: MediaItem,
    pub year: Option<String>,
    pub thumbnail: Option<String>,
    /// Tracks as MediaItem (canonical type)
    pub tracks: Vec<MediaItem>,
    pub more_by_artist: Vec<MediaItem>,
}

/// Detailed artist info (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistDetailsData {
    pub id: String,
    pub name: String,
    pub subscribers: Option<String>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    /// Top songs as MediaItem (canonical type)
    pub top_songs: Vec<MediaItem>,
    pub albums: Vec<MediaItem>,
    pub singles: Vec<MediaItem>,
    pub related_artists: Vec<MediaItem>,
}

// Conversions from domain types to protocol types (using MediaItem as canonical
// type)

impl From<crate::backends::youtube::details::ArtistRef> for MediaItem {
    fn from(a: crate::backends::youtube::details::ArtistRef) -> Self {
        use crate::domain::media_item::{Artist, BackendExtension};
        MediaItem::Artist(Artist {
            id: a.id,
            name: a.name,
            subscribers: None,
            thumbnail: a.thumbnail,
            description: None,
            backend: BackendExtension::None,
        })
    }
}

impl From<crate::backends::youtube::details::AlbumRef> for MediaItem {
    fn from(a: crate::backends::youtube::details::AlbumRef) -> Self {
        use crate::domain::media_item::{Album, BackendExtension};
        MediaItem::Album(Album {
            id: a.id,
            title: a.title,
            artist: None,
            year: a.year.and_then(|y| y.parse().ok()),
            track_count: None,
            thumbnail: a.thumbnail,
            explicit: false,
            backend: BackendExtension::None,
        })
    }
}

impl From<crate::backends::youtube::details::PlaylistRef> for MediaItem {
    fn from(p: crate::backends::youtube::details::PlaylistRef) -> Self {
        use crate::domain::media_item::{BackendExtension, Playlist};
        MediaItem::Playlist(Playlist {
            id: p.id,
            title: p.title,
            author: p.subtitle,
            track_count: None,
            thumbnail: p.thumbnail,
            description: None,
            backend: BackendExtension::None,
        })
    }
}

impl From<crate::backends::youtube::details::PlaylistDetails> for PlaylistDetailsData {
    fn from(p: crate::backends::youtube::details::PlaylistDetails) -> Self {
        Self {
            id: p.id,
            title: p.title,
            artist: p.artist,
            year: p.year,
            thumbnail: p.thumbnail,
            track_count: p.track_count,
            duration_text: p.duration_text,
            tracks: p.tracks.into_iter().map(MediaItem::from).collect(),
            featured_artists: p.featured_artists.into_iter().map(MediaItem::from).collect(),
            related_playlists: p.related_playlists.into_iter().map(MediaItem::from).collect(),
        }
    }
}

impl From<crate::backends::youtube::details::AlbumDetails> for AlbumDetailsData {
    fn from(a: crate::backends::youtube::details::AlbumDetails) -> Self {
        Self {
            id: a.id,
            title: a.title,
            artist: MediaItem::from(a.artist),
            year: a.year,
            thumbnail: a.thumbnail,
            tracks: a.tracks.into_iter().map(MediaItem::from).collect(),
            more_by_artist: a.more_by_artist.into_iter().map(MediaItem::from).collect(),
        }
    }
}

impl From<crate::backends::youtube::details::ArtistDetails> for ArtistDetailsData {
    fn from(a: crate::backends::youtube::details::ArtistDetails) -> Self {
        Self {
            id: a.id,
            name: a.name,
            subscribers: a.subscribers,
            description: a.description,
            thumbnail: a.thumbnail,
            top_songs: a.top_songs.into_iter().map(MediaItem::from).collect(),
            albums: a.albums.into_iter().map(MediaItem::from).collect(),
            singles: a.singles.into_iter().map(MediaItem::from).collect(),
            related_artists: a.related_artists.into_iter().map(MediaItem::from).collect(),
        }
    }
}

// Conversions from protocol types back to domain types
// Helper to convert MediaItem back to Song
fn media_item_to_song(m: &MediaItem) -> Song {
    Song::from(m.clone())
}

// Helper to convert MediaItem to ArtistRef
fn media_item_to_artist_ref(m: &MediaItem) -> crate::backends::youtube::details::ArtistRef {
    use crate::{backends::youtube::details::ArtistRef, domain::media_item::Displayable};
    ArtistRef {
        id: m.id().to_string(),
        name: m.title().to_string(),
        thumbnail: m.thumbnail_url().map(|s| s.to_string()),
    }
}

// Helper to convert MediaItem to AlbumRef
fn media_item_to_album_ref(m: &MediaItem) -> crate::backends::youtube::details::AlbumRef {
    use crate::{backends::youtube::details::AlbumRef, domain::media_item::Displayable};
    AlbumRef {
        id: m.id().to_string(),
        title: m.title().to_string(),
        year: match m {
            MediaItem::Album(a) => a.year.map(|y| y.to_string()),
            _ => None,
        },
        thumbnail: m.thumbnail_url().map(|s| s.to_string()),
    }
}

// Helper to convert MediaItem to PlaylistRef
fn media_item_to_playlist_ref(m: &MediaItem) -> crate::backends::youtube::details::PlaylistRef {
    use crate::{backends::youtube::details::PlaylistRef, domain::media_item::Displayable};
    PlaylistRef {
        id: m.id().to_string(),
        title: m.title().to_string(),
        thumbnail: m.thumbnail_url().map(|s| s.to_string()),
        subtitle: m.subtitle().map(|s| s.to_string()),
    }
}

impl PlaylistDetailsData {
    pub fn to_details(&self) -> crate::backends::youtube::details::PlaylistDetails {
        use crate::backends::youtube::details::*;
        PlaylistDetails {
            id: self.id.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            year: self.year.clone(),
            thumbnail: self.thumbnail.clone(),
            track_count: self.track_count,
            duration_text: self.duration_text.clone(),
            tracks: self.tracks.iter().map(media_item_to_song).collect(),
            featured_artists: self.featured_artists.iter().map(media_item_to_artist_ref).collect(),
            related_playlists: self
                .related_playlists
                .iter()
                .map(media_item_to_playlist_ref)
                .collect(),
        }
    }
}

impl AlbumDetailsData {
    pub fn to_details(&self) -> crate::backends::youtube::details::AlbumDetails {
        use crate::backends::youtube::details::*;
        AlbumDetails {
            id: self.id.clone(),
            title: self.title.clone(),
            artist: media_item_to_artist_ref(&self.artist),
            year: self.year.clone(),
            thumbnail: self.thumbnail.clone(),
            tracks: self.tracks.iter().map(media_item_to_song).collect(),
            more_by_artist: self.more_by_artist.iter().map(media_item_to_album_ref).collect(),
        }
    }
}

impl ArtistDetailsData {
    pub fn to_details(&self) -> crate::backends::youtube::details::ArtistDetails {
        use crate::backends::youtube::details::*;
        ArtistDetails {
            id: self.id.clone(),
            name: self.name.clone(),
            subscribers: self.subscribers.clone(),
            description: self.description.clone(),
            thumbnail: self.thumbnail.clone(),
            top_songs: self.top_songs.iter().map(media_item_to_song).collect(),
            albums: self.albums.iter().map(media_item_to_album_ref).collect(),
            singles: self.singles.iter().map(media_item_to_album_ref).collect(),
            related_artists: self.related_artists.iter().map(media_item_to_artist_ref).collect(),
        }
    }
}

/// Frame-based protocol for reading/writing messages over socket
pub mod framing {
    use std::io::{Read, Write};

    use anyhow::{Context, Result};
    use serde::{Deserialize, Serialize};

    /// Write a message with length prefix
    pub fn write_message<W: Write, T: Serialize>(writer: &mut W, msg: &T) -> Result<()> {
        let json = serde_json::to_vec(msg)?;
        let len = json.len() as u32;
        log::trace!("framing::write_message - serialized {} bytes", len);
        writer.write_all(&len.to_le_bytes())?;
        writer.write_all(&json)?;
        writer.flush()?;
        log::trace!("framing::write_message - flushed successfully");
        Ok(())
    }

    /// Read a message with length prefix
    pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> Result<T> {
        let mut len_bytes = [0u8; 4];
        log::trace!("framing::read_message - reading length bytes");
        reader.read_exact(&mut len_bytes).context("Failed to read message length")?;
        let len = u32::from_le_bytes(len_bytes) as usize;
        log::trace!("framing::read_message - message length: {} bytes", len);

        if len > 10 * 1024 * 1024 {
            anyhow::bail!("Message too large: {} bytes", len);
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).context("Failed to read message body")?;
        log::trace!("framing::read_message - read {} bytes, deserializing", len);
        serde_json::from_slice(&buf).context("Failed to parse message")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_serialization() {
        let cmd = ServerCommand::Search { query: "test".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: ServerCommand = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerCommand::Search { query } => assert_eq!(query, "test"),
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_response_serialization() {
        let resp = ServerResponse::Status(StatusData {
            state: "play".into(),
            volume: 50,
            elapsed_ms: Some(1000),
            duration_ms: Some(180000),
            playlist_length: 5,
            current_pos: Some(0),
            current_id: Some(1),
            repeat: "off".into(),
            shuffle: false,
            next_queue_pos: None,
        });
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ServerResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerResponse::Status(s) => {
                assert_eq!(s.state, "play");
                assert_eq!(s.volume, 50);
            }
            _ => panic!("Wrong response type"),
        }
    }

    #[test]
    fn test_framing_roundtrip() {
        use std::io::Cursor;

        let cmd = ServerCommand::PlayPos(5);
        let mut buf = Vec::new();
        framing::write_message(&mut buf, &cmd).unwrap();

        let mut cursor = Cursor::new(buf);
        let parsed: ServerCommand = framing::read_message(&mut cursor).unwrap();
        match parsed {
            ServerCommand::PlayPos(pos) => assert_eq!(pos, 5),
            _ => panic!("Wrong command type"),
        }
    }
}
