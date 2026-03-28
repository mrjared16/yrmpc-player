//! Queue Entry - Represents an item in the playback queue
//!
//! This struct wraps a playable item with queue-specific metadata
//! such as backend-assigned queue ID.

use super::search::PlayableItem;

/// An entry in the playback queue.
///
/// This wraps a `PlayableItem` with additional queue-specific metadata.
/// The position is implicit (index in the containing `Vec<QueueEntry>`).
///
/// # Type Safety
///
/// By wrapping `PlayableItem` (not `SearchItem`), we ensure at compile time
/// that only playable content (songs/videos) can be in the queue - not
/// artists, albums, playlists, or headers.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueEntry {
    /// The playable content (song or video)
    pub content: PlayableItem,

    /// Backend-specific queue ID.
    /// - MPD: The queue song ID assigned by MPD
    /// - YouTube (local queue): None (position = Vec index)
    pub backend_id: Option<u32>,
}

impl QueueEntry {
    /// Create a new queue entry from a playable item
    pub fn new(content: PlayableItem) -> Self {
        Self { content, backend_id: None }
    }

    /// Create a new queue entry with a backend ID
    pub fn with_backend_id(content: PlayableItem, backend_id: u32) -> Self {
        Self { content, backend_id: Some(backend_id) }
    }

    /// Get the video ID for playback
    pub fn video_id(&self) -> &str {
        self.content.video_id()
    }

    /// Get the ContentUri for this queue entry
    pub fn content_uri(&self) -> super::content_uri::ContentUri {
        self.content.content_uri()
    }

    /// Check if this entry has a confirmed backend ID
    pub fn is_confirmed(&self) -> bool {
        self.backend_id.is_some()
    }
}

// Forward display traits to the inner content
impl super::search::Displayable for QueueEntry {
    fn primary_line(&self) -> &str {
        super::search::Displayable::primary_line(&self.content)
    }

    fn secondary_line(&self) -> Option<String> {
        super::search::Displayable::secondary_line(&self.content)
    }

    fn thumbnail(&self) -> Option<&str> {
        super::search::Displayable::thumbnail(&self.content)
    }

    fn type_icon(&self) -> &'static str {
        super::search::Displayable::type_icon(&self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::search::{PlayableItem, SongItem};

    #[test]
    fn test_queue_entry_creation() {
        let song = SongItem {
            video_id: "abc123".to_string(),
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: Some("Test Album".to_string()),
            duration: Some(std::time::Duration::from_secs(180)),
            thumbnail: None,
            explicit: false,
            radio_playlist_id: None,
            search_key: Default::default(),
        };

        let entry = QueueEntry::new(PlayableItem::Song(song));

        assert_eq!(entry.video_id(), "abc123");
        assert_eq!(entry.backend_id, None);
        assert!(!entry.is_confirmed());
    }

    #[test]
    fn test_queue_entry_with_backend_id() {
        let song = SongItem::default();
        let entry = QueueEntry::with_backend_id(PlayableItem::Song(song), 42);

        assert_eq!(entry.backend_id, Some(42));
        assert!(entry.is_confirmed());
    }

    #[test]
    fn test_content_uri() {
        let song = SongItem { video_id: "dQw4w9WgXcQ".to_string(), ..Default::default() };

        let entry = QueueEntry::new(PlayableItem::Song(song));
        let uri = entry.content_uri();

        assert_eq!(uri.as_str(), "yt:v:dQw4w9WgXcQ");
        assert!(uri.is_youtube());
        assert!(uri.is_playable());
    }
}
