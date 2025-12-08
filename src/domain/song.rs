use std::collections::HashMap;
use std::time::Duration;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Domain model for a song/track
/// Backend-agnostic - can be populated from MPD, YouTube Music, Spotify, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    /// Unique ID for this song in the current context
    /// Option because not all backends provide IDs
    pub id: Option<u32>,

    /// URI to the song (local path, URL, or backend-specific identifier)
    /// Examples:
    /// - MPD: "Music/Artist/Album/Track.flac"
    /// - YouTube: "dQw4w9WgXcQ" (video ID)
    /// - Browse: "artist:UC12345", "album:MPREb_xyz"
    #[serde(alias = "file")]  // Backward compat for existing config/cache files
    pub uri: String,

    /// Duration of the song
    pub duration: Option<Duration>,

    /// Metadata tags (title, artist, album, etc.)
    /// Using Vec<String> for values to support multiple values per tag
    pub metadata: HashMap<String, Vec<String>>,

    /// When the song was last modified (if applicable)
    pub last_modified: Option<DateTime<Utc>>,

    /// When the song was added to the library/playlist (if applicable)
    pub added: Option<DateTime<Utc>>,
}

impl Default for Song {
    fn default() -> Self {
        Self {
            id: None,
            uri: String::new(),
            duration: None,
            metadata: HashMap::new(),
            last_modified: None,
            added: None,
        }
    }
}

impl Song {
    /// Get the title from metadata, or URI as fallback
    pub fn title(&self) -> &str {
        self.metadata
            .get("title")
            .and_then(|v| v.first())
            .map(|s| s.as_str())
            .unwrap_or(&self.uri)
    }

    /// Get the artist from metadata
    pub fn artist(&self) -> Option<&str> {
        self.metadata
            .get("artist")
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }

    /// Get the album from metadata
    pub fn album(&self) -> Option<&str> {
        self.metadata
            .get("album")
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }
}

/// Conversion from MPD Song to domain Song
impl From<crate::mpd::commands::current_song::Song> for Song {
    fn from(mpd_song: crate::mpd::commands::current_song::Song) -> Self {
        use crate::mpd::commands::metadata_tag::MetadataTag;
        
        // Convert metadata from HashMap<String, MetadataTag> to HashMap<String, Vec<String>>
        let metadata = mpd_song.metadata.into_iter().map(|(key, tag)| {
            // Convert MetadataTag enum to Vec<String>
            let values = match tag {
                MetadataTag::Single(s) => vec![s],
                MetadataTag::Multiple(v) => v,
            };
            (key, values)
        }).collect();

        Self {
            id: Some(mpd_song.id),
            uri: mpd_song.file,  // MPD uses 'file' as the field name
            duration: mpd_song.duration,
            metadata,
            last_modified: Some(mpd_song.last_modified),
            added: mpd_song.added,
        }
    }
}

// Implement ListItemDisplay for rich list rendering
use crate::domain::display::ListItemDisplay;
use ratatui::style::{Color, Style};
use std::borrow::Cow;

impl Song {
    /// Get the item type from metadata (used for icon/style selection, focusability checks).
    pub fn item_type(&self) -> Option<&str> {
        self.metadata.get("type")
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }
}

impl ListItemDisplay for Song {
    fn primary_text(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.title())
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        // For browsable items (artist/album/playlist), check subtitle first
        // This contains type-specific metadata like "4.69K subscribers" or "2024"
        if let Some(subtitle) = self.metadata.get("subtitle").and_then(|v| v.first()) {
            return Some(Cow::Borrowed(subtitle.as_str()));
        }
        
        // For playable items (songs/videos), use artist · album
        match (self.artist(), self.album()) {
            (Some(a), Some(b)) => Some(Cow::Owned(format!("{} · {}", a, b))),
            (Some(a), None) => Some(Cow::Borrowed(a)),
            (None, Some(b)) => Some(Cow::Borrowed(b)),
            (None, None) => None,
        }
    }

    fn thumbnail_url(&self) -> Option<&str> {
        self.metadata.get("thumbnail").and_then(|v| v.first()).map(|s| s.as_str())
    }

    fn type_icon(&self) -> &str {
        match self.item_type() {
            Some("artist") => "🎤",
            Some("album") => "💿",
            Some("playlist") => "📁",
            Some("video") => "🎬",
            Some("header") => "─",
            _ => "🎵",
        }
    }

    fn icon_style(&self) -> Style {
        // Type-specific colors per ui-ux-provised.md 4.1
        match self.item_type() {
            Some("artist") => Style::default().fg(Color::Cyan),
            Some("album") => Style::default().fg(Color::Yellow),
            Some("playlist") => Style::default().fg(Color::Magenta),
            Some("video") => Style::default().fg(Color::Red),
            Some("header") => Style::default().fg(Color::DarkGray),
            _ => Style::default().fg(Color::White),
        }
    }

    fn duration_text(&self) -> Option<Cow<'_, str>> {
        self.duration.map(|d| {
            let secs = d.as_secs();
            let mins = secs / 60;
            let secs = secs % 60;
            Cow::Owned(format!("{}:{:02}", mins, secs))
        })
    }

    fn is_header(&self) -> bool {
        self.item_type() == Some("header")
    }
}

