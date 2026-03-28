use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::display::SearchKey;
use crate::shared::string_util::fold_for_match;

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
    #[serde(alias = "file")] // Backward compat for existing config/cache files
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

    /// Folded search data for fast `/` matching.
    #[serde(skip, default)]
    pub search_key: SearchKey,
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
            search_key: SearchKey::default(),
        }
    }
}

impl Song {
    /// Get the title from metadata, or URI as fallback
    pub fn title(&self) -> &str {
        self.metadata.get("title").and_then(|v| v.first()).map(|s| s.as_str()).unwrap_or(&self.uri)
    }

    /// Get the artist from metadata
    pub fn artist(&self) -> Option<&str> {
        self.metadata.get("artist").and_then(|v| v.first()).map(|s| s.as_str())
    }

    /// Get the album from metadata
    pub fn album(&self) -> Option<&str> {
        self.metadata.get("album").and_then(|v| v.first()).map(|s| s.as_str())
    }

    fn computed_search_key(&self) -> SearchKey {
        let secondary = self
            .metadata
            .get("subtitle")
            .and_then(|v| v.first())
            .map(|subtitle| fold_for_match(subtitle))
            .or_else(|| match (self.artist(), self.album()) {
                (Some(a), Some(b)) => Some(fold_for_match(&format!("{} · {}", a, b))),
                (Some(a), None) => Some(fold_for_match(a)),
                (None, Some(b)) => Some(fold_for_match(b)),
                (None, None) => None,
            });
        SearchKey::new(fold_for_match(self.title()), secondary)
    }

    pub fn refresh_search_key(&mut self) {
        self.search_key = self.computed_search_key();
    }

    #[must_use]
    pub fn with_search_key(mut self) -> Self {
        self.refresh_search_key();
        self
    }
}

/// Conversion from MPD Song to domain Song
impl From<crate::mpd::commands::current_song::Song> for Song {
    fn from(mpd_song: crate::mpd::commands::current_song::Song) -> Self {
        use crate::mpd::commands::metadata_tag::MetadataTag;

        // Convert metadata from HashMap<String, MetadataTag> to HashMap<String,
        // Vec<String>>
        let metadata = mpd_song
            .metadata
            .into_iter()
            .map(|(key, tag)| {
                // Convert MetadataTag enum to Vec<String>
                let values = match tag {
                    MetadataTag::Single(s) => vec![s],
                    MetadataTag::Multiple(v) => v,
                };
                (key, values)
            })
            .collect();

        Self {
            id: Some(mpd_song.id),
            uri: mpd_song.file, // MPD uses 'file' as the field name
            duration: mpd_song.duration,
            metadata,
            last_modified: Some(mpd_song.last_modified),
            added: mpd_song.added,
            search_key: SearchKey::default(),
        }
        .with_search_key()
    }
}

// Implement ListItemDisplay for rich list rendering
use std::borrow::Cow;

use ratatui::style::{Color, Style};

use crate::domain::display::ListItemDisplay;

impl Song {
    /// Get the item type from metadata (used for icon/style selection,
    /// focusability checks).
    pub fn item_type(&self) -> Option<&str> {
        self.metadata.get("type").and_then(|v| v.first()).map(|s| s.as_str())
    }

    /// Get the browse ID for navigable items (albums, artists, playlists).
    /// This is the ID used to fetch details from the backend.
    pub fn browse_id(&self) -> Option<&str> {
        self.metadata.get("browse_id").and_then(|v| v.first()).map(|s| s.as_str())
    }

    /// Get the radio playlist ID for auto-population (if available).
    /// This ID is used to fetch similar tracks when the queue is running low.
    pub fn radio_playlist_id(&self) -> Option<&str> {
        self.metadata.get("radio_playlist_id").and_then(|v| v.first()).map(|s| s.as_str())
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
            Some("header") => "─", // Header shows dash
            Some("song") => "🎵",  // Explicit song type shows music note
            _ => "",               // Unknown types or no metadata show nothing
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

    fn search_key(&self) -> SearchKey {
        if self.search_key.is_empty() {
            self.computed_search_key()
        } else {
            self.search_key.clone()
        }
    }

    fn matches_folded_query(&self, folded_query: &str) -> bool {
        if self.search_key.is_empty() {
            self.computed_search_key().matches(folded_query)
        } else {
            self.search_key.matches(folded_query)
        }
    }
}

// =============================================================================
// TESTS - Verify Song metadata → icon/header mapping works correctly
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::ListItemDisplay;

    /// Helper to create Song with specific type metadata
    fn song_with_type(type_str: &str) -> Song {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("type".into(), vec![type_str.into()]);
        metadata.insert("title".into(), vec!["Test".into()]);
        Song {
            id: None,
            uri: "test123".into(),
            duration: None,
            metadata,
            last_modified: None,
            added: None,
            search_key: SearchKey::default(),
        }
    }

    #[test]
    fn song_with_artist_type_shows_artist_icon() {
        let song = song_with_type("artist");
        assert_eq!(song.type_icon(), "🎤", "Artist should show microphone icon");
    }

    #[test]
    fn song_with_album_type_shows_album_icon() {
        let song = song_with_type("album");
        assert_eq!(song.type_icon(), "💿", "Album should show disc icon");
    }

    #[test]
    fn song_with_playlist_type_shows_playlist_icon() {
        let song = song_with_type("playlist");
        assert_eq!(song.type_icon(), "📁", "Playlist should show folder icon");
    }

    #[test]
    fn song_with_video_type_shows_video_icon() {
        let song = song_with_type("video");
        assert_eq!(song.type_icon(), "🎬", "Video should show clapperboard icon");
    }

    #[test]
    fn song_with_header_type_shows_dash_icon() {
        let song = song_with_type("header");
        assert_eq!(song.type_icon(), "─", "Header should show dash");
        assert!(song.is_header(), "Header type should be identified as header");
    }

    #[test]
    fn song_with_explicit_song_type_shows_music_icon() {
        let song = song_with_type("song");
        assert_eq!(song.type_icon(), "🎵", "Explicit song type should show music note");
    }

    #[test]
    fn song_with_unknown_type_shows_nothing() {
        let song = song_with_type("unknown_garbage");
        assert_eq!(song.type_icon(), "", "Unknown type should show nothing");
    }

    #[test]
    fn song_without_type_metadata_shows_nothing() {
        let song = Song::default();
        assert_eq!(song.type_icon(), "", "No type metadata should show nothing");
    }

    #[test]
    fn song_thumbnail_url_returns_metadata_value() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("thumbnail".into(), vec!["https://example.com/cover.jpg".into()]);
        let song = Song {
            id: None,
            uri: "test".into(),
            duration: None,
            metadata,
            last_modified: None,
            added: None,
            search_key: SearchKey::default(),
        };
        assert_eq!(song.thumbnail_url(), Some("https://example.com/cover.jpg"));
    }
}
