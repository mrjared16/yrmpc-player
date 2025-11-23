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
    
    /// URI/path to the song (can be local path, URL, or backend-specific identifier)
    pub file: String,
    
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
            file: String::new(),
            duration: None,
            metadata: HashMap::new(),
            last_modified: None,
            added: None,
        }
    }
}

impl Song {
    /// Get the title from metadata, or filename as fallback
    pub fn title(&self) -> &str {
        self.metadata
            .get("title")
            .and_then(|v| v.first())
            .map(|s| s.as_str())
            .unwrap_or(&self.file)
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
            file: mpd_song.file,
            duration: mpd_song.duration,
            metadata,
            last_modified: Some(mpd_song.last_modified),
            added: mpd_song.added,
        }
    }
}
