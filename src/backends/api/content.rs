//! Content types and capabilities.
//!
//! Defines the universal content model used across all backends.

use std::time::Duration;

// Re-export ContentType from domain (single source of truth)
pub use crate::domain::ContentType;

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
///
/// # Design Principle
///
/// > "Ask CAN you do X, not ARE you backend Y"
///
/// ```ignore
/// // ✅ CORRECT
/// if client.supports(Capability::Playlists) { ... }
///
/// // ❌ WRONG
/// if let Some(mpd) = client.as_mpd() { ... }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    // =========================================================================
    // Optional Common Features (multiple backends may support)
    // =========================================================================
    
    /// Can list/view user playlists
    Playlists,
    /// Can create new playlists
    PlaylistCreate,
    /// Can edit existing playlists (add/remove tracks)
    PlaylistEdit,
    /// Fetch song lyrics
    Lyrics,
    /// Start radio/mix from seed content
    Radio,
    /// Like/dislike tracks (favorites)
    UserLikes,
    /// Search autocomplete suggestions
    SearchSuggestions,
    /// Rich metadata (thumbnails, full artist info)
    RichMetadata,
    
    // =========================================================================
    // Queue Behavior Modes (local logic, flag indicates if implemented)
    // =========================================================================
    
    /// Stop playback after current track finishes
    SingleMode,
    /// Remove tracks from queue after playing
    ConsumeMode,
    
    // =========================================================================
    // Audio Effects
    // =========================================================================
    
    /// Crossfade between tracks
    Crossfade,
    /// Gapless playback (no silence between tracks)
    GaplessPlayback,
    
    // =========================================================================
    // Backend-Specific (only one backend has these)
    // =========================================================================
    
    /// MPD audio outputs control
    MpdOutputs,
    /// MPD database rescan
    MpdDatabase,
    /// MPD stickers (rating, tags)
    MpdStickers,
    /// MPD partitions (multi-room audio)
    MpdPartitions,
    
    // =========================================================================
    // Legacy (deprecated - use specific names above)
    // =========================================================================
    
    #[deprecated(since = "0.1.0", note = "Use Playlists instead")]
    SavedPlaylists,
    #[deprecated(since = "0.1.0", note = "Use MpdStickers instead")]
    Stickers,
    #[deprecated(since = "0.1.0", note = "Use MpdOutputs instead")]
    Outputs,
    #[deprecated(since = "0.1.0", note = "Use MpdDatabase instead")]
    DatabaseManagement,
    #[deprecated(since = "0.1.0", note = "Use MpdOutputs instead")]
    OutputControl,
    #[deprecated(since = "0.1.0", note = "Use MpdPartitions instead")]
    Partitions,
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
