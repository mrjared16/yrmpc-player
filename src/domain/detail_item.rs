//! DetailItem - Domain items for navigation stacks.
//!
//! This enum represents **actionable content** that can appear in a navigation
//! list:
//! - Songs (playable)
//! - ContentRefs (navigable - albums, artists, playlists)
//!
//! ## Architecture Note
//!
//! **Presentation-only items** (headers, spacers) belong in the UI layer.
//! Use [`ui::widgets::list_item::ListItem`] for list rendering, which wraps
//! `DetailItem` and adds non-actionable variants.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ UI Layer (ListItem)                                                 │
//! │   - ListItem::Content(DetailItem) - actionable content              │
//! │   - ListItem::Header(String)      - section headers (non-focusable) │
//! │   - ListItem::Spacer              - visual spacing                  │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Domain Layer (DetailItem)                                           │
//! │   - DetailItem::Song(Song)        - playable content                │
//! │   - DetailItem::Ref(ContentRef)   - navigable reference             │
//! └─────────────────────────────────────────────────────────────────────┘
//!
//! # Design Decisions
//!
//! 1. **Type-safe navigation**: No more `metadata.get("type") == "album"` checks
//! 2. **Single stack type**: `NavStack<DetailItem>` works for search results AND detail views
//! 3. **Clean ListItemDisplay impl**: Each variant renders appropriately
//! 4. **Extensible**: Easy to add Video, Podcast, etc. in the future
//! 5. **Clean separation**: Headers are NEVER in DetailItem - they live in ListItem (UI layer)
//!
//! # Usage
//!
//! Build lists for display using `Vec<ListItem>`:
//! ```rust,ignore
//! use crate::ui::widgets::list_item::ListItem;
//!
//! let items: Vec<ListItem> = vec![
//!     ListItem::Header("Top Songs".into()),
//!     ListItem::Content(DetailItem::Song(song1)),
//!     ListItem::Content(DetailItem::Song(song2)),
//! ];
//! ```
//!
//! Extract actionable items for Intent/Selection:
//! ```rust,ignore
//! let actionable: Vec<DetailItem> = items
//!     .iter()
//!     .filter_map(|item| item.as_content())
//!     .cloned()
//!     .collect();
//! ```

use std::borrow::Cow;

use ratatui::style::{Color, Style};

use super::{
    content::{ContentRef, ContentType},
    display::ListItemDisplay,
    search::{BrowsableItem, PlayableItem, SearchItem},
    song::Song,
};

/// A unified item type for navigation lists.
///
/// Contains only **actionable content** (Song, Ref).
/// Headers are in the UI layer via `ListItem::Header`.
#[derive(Debug, Clone)]
pub enum DetailItem {
    /// A playable song
    Song(Song),
    /// A navigable reference to content (album, artist, playlist)
    Ref(ContentRef),
}

impl DetailItem {
    /// Create a song item.
    pub fn song(song: Song) -> Self {
        Self::Song(song)
    }

    /// Create a reference item from ContentRef.
    pub fn content_ref(reference: ContentRef) -> Self {
        Self::Ref(reference)
    }

    /// Create an artist reference.
    pub fn artist(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Ref(ContentRef::artist(id, name))
    }

    /// Create an album reference.
    pub fn album(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::Ref(ContentRef::album(id, title))
    }

    /// Create a playlist reference.
    pub fn playlist(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::Ref(ContentRef::playlist(id, title))
    }

    /// Create a header item (placeholder for UI bridging).
    ///
    /// Note: In the new architecture, headers should be in ListItem::Header,
    /// but sometimes we need to round-trip through DetailItem.
    pub fn header(title: impl Into<String>) -> Self {
        Self::Ref(ContentRef {
            id: String::new(),
            name: title.into(),
            content_type: ContentType::Header,
            thumbnail: None,
            subtitle: None,
        })
    }

    /// Check if this is a song.
    pub fn is_song(&self) -> bool {
        matches!(self, Self::Song(_))
    }

    /// Check if this is a navigable reference.
    pub fn is_ref(&self) -> bool {
        matches!(self, Self::Ref(_))
    }

    /// Check if this item is navigable (can be drilled into).
    pub fn is_navigable(&self) -> bool {
        match self {
            Self::Ref(r) => matches!(
                r.content_type,
                ContentType::Album | ContentType::Artist | ContentType::Playlist
            ),
            _ => false,
        }
    }

    /// Check if this item is playable.
    pub fn is_playable(&self) -> bool {
        matches!(self, Self::Song(_))
    }

    /// Headers created via `DetailItem::header()` are NOT focusable.
    /// Other DetailItems (Song, Ref with non-Header type) ARE focusable.
    pub fn is_focusable(&self) -> bool {
        // Headers are not focusable
        if let Self::Ref(r) = self {
            return !matches!(r.content_type, ContentType::Header);
        }
        true
    }

    /// Get the ID for navigation (if applicable).
    pub fn navigation_id(&self) -> Option<&str> {
        match self {
            Self::Ref(r) => Some(&r.id),
            Self::Song(s) => Some(&s.uri),
        }
    }

    /// Get the content type for navigation (if applicable).
    pub fn content_type(&self) -> Option<ContentType> {
        match self {
            Self::Ref(r) => Some(r.content_type),
            Self::Song(_) => Some(ContentType::Track),
        }
    }

    /// Get as song reference.
    pub fn as_song(&self) -> Option<&Song> {
        match self {
            Self::Song(s) => Some(s),
            _ => None,
        }
    }

    /// Get as content reference.
    pub fn as_content_ref(&self) -> Option<&ContentRef> {
        match self {
            Self::Ref(r) => Some(r),
            _ => None,
        }
    }
}

// =============================================================================
// LIST ITEM DISPLAY IMPLEMENTATION
// =============================================================================

impl ListItemDisplay for DetailItem {
    fn primary_text(&self) -> Cow<'_, str> {
        match self {
            Self::Song(song) => song.primary_text(),
            Self::Ref(r) => Cow::Borrowed(&r.name),
        }
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Song(song) => song.secondary_text(),
            Self::Ref(r) => r.subtitle.as_ref().map(|s| Cow::Borrowed(s.as_str())),
        }
    }

    fn thumbnail_url(&self) -> Option<&str> {
        match self {
            Self::Song(song) => song.thumbnail_url(),
            Self::Ref(r) => r.thumbnail.as_deref(),
        }
    }

    fn type_icon(&self) -> &str {
        match self {
            // DetailItem::Song is always a playable track - use music note
            // Don't delegate to song.type_icon() which relies on metadata["type"]
            Self::Song(_) => "🎵",
            Self::Ref(r) => match r.content_type {
                ContentType::Artist => "🎤",
                ContentType::Album => "💿",
                ContentType::Playlist => "📁",
                ContentType::Directory => "📂",
                ContentType::Video => "🎬",
                ContentType::Track => "🎵",
                ContentType::Header => "─",
            },
        }
    }

    fn icon_style(&self) -> Style {
        match self {
            // DetailItem::Song is always a playable track - use yellow
            Self::Song(_) => Style::default().fg(Color::Yellow),
            Self::Ref(r) => match r.content_type {
                ContentType::Artist => Style::default().fg(Color::Cyan),
                ContentType::Album => Style::default().fg(Color::Magenta),
                ContentType::Playlist => Style::default().fg(Color::Green),
                ContentType::Directory => Style::default().fg(Color::Blue),
                ContentType::Video => Style::default().fg(Color::Red),
                ContentType::Track => Style::default().fg(Color::Yellow),
                ContentType::Header => Style::default().fg(Color::DarkGray),
            },
        }
    }

    fn duration_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Song(song) => song.duration_text(),
            _ => None,
        }
    }

    fn is_playing(&self) -> bool {
        match self {
            Self::Song(song) => song.is_playing(),
            _ => false,
        }
    }

    fn is_header(&self) -> bool {
        // Only return true if it's a Ref with ContentType::Header
        if let Self::Ref(r) = self {
            return matches!(r.content_type, ContentType::Header);
        }
        false
    }

    fn is_focusable(&self) -> bool {
        // Headers are not focusable
        !self.is_header()
    }
}

// =============================================================================
// CONVERSION FROM SearchItem (type-safe, no metadata parsing)
// =============================================================================

impl From<SearchItem> for DetailItem {
    fn from(item: SearchItem) -> Self {
        match item {
            SearchItem::Playable(p) => DetailItem::from(p),
            SearchItem::Browsable(b) => DetailItem::from(b),
        }
    }
}

impl From<PlayableItem> for DetailItem {
    fn from(item: PlayableItem) -> Self {
        match item {
            PlayableItem::Song(s) => {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("title".into(), vec![s.title.clone()]);
                metadata.insert("artist".into(), vec![s.artist.clone()]);
                if let Some(ref album) = s.album {
                    metadata.insert("album".into(), vec![album.clone()]);
                }
                if let Some(ref thumb) = s.thumbnail {
                    metadata.insert("thumbnail".into(), vec![thumb.clone()]);
                }

                DetailItem::Song(Song {
                    id: None,
                    uri: s.video_id.clone(),
                    duration: s.duration,
                    metadata,
                    last_modified: None,
                    added: None,
                })
            }
            PlayableItem::Video(v) => {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("title".into(), vec![v.title.clone()]);
                metadata.insert("artist".into(), vec![v.channel.clone()]);
                if let Some(ref thumb) = v.thumbnail {
                    metadata.insert("thumbnail".into(), vec![thumb.clone()]);
                }

                DetailItem::Song(Song {
                    id: None,
                    uri: v.video_id.clone(),
                    duration: v.duration,
                    metadata,
                    last_modified: None,
                    added: None,
                })
            }
        }
    }
}

impl From<BrowsableItem> for DetailItem {
    fn from(item: BrowsableItem) -> Self {
        match item {
            BrowsableItem::Artist(a) => DetailItem::Ref(ContentRef {
                id: a.browse_id.clone().unwrap_or_default(),
                name: a.name.clone(),
                content_type: ContentType::Artist,
                thumbnail: a.thumbnail.clone(),
                subtitle: a.subscribers.clone(),
            }),
            BrowsableItem::Album(a) => DetailItem::Ref(ContentRef {
                id: a.album_id.clone(),
                name: a.title.clone(),
                content_type: ContentType::Album,
                thumbnail: a.thumbnail.clone(),
                subtitle: Some(a.artist.clone()),
            }),
            BrowsableItem::Playlist(p) => DetailItem::Ref(ContentRef {
                id: p.playlist_id.clone(),
                name: p.title.clone(),
                content_type: ContentType::Playlist,
                thumbnail: p.thumbnail.clone(),
                subtitle: Some(p.author.clone()),
            }),
        }
    }
}

// =============================================================================
// CONVERSION FROM SONG (simplified - no more metadata type parsing)
// =============================================================================

impl From<Song> for DetailItem {
    fn from(song: Song) -> Self {
        // Simple conversion: Song is always a playable Song variant
        // No more metadata["type"] parsing - that coupling is eliminated
        DetailItem::Song(song)
    }
}

// =============================================================================
// CONVERSION FROM MEDIAITEM (strongly-typed, no data loss)
// =============================================================================

use crate::domain::media_item::{
    Album as MediaAlbum, Artist as MediaArtist, MediaItem, Playlist as MediaPlaylist, Track,
};

impl From<MediaItem> for DetailItem {
    fn from(media: MediaItem) -> Self {
        match media {
            MediaItem::Track(t) => {
                // Convert to Song for backward compat with existing UI
                DetailItem::Song(Song::from(MediaItem::Track(t)))
            }
            MediaItem::Artist(a) => {
                let mut item = DetailItem::artist(&a.id, &a.name);
                if let DetailItem::Ref(ref mut r) = item {
                    r.subtitle = a.subscribers;
                    r.thumbnail = a.thumbnail;
                }
                item
            }
            MediaItem::Album(a) => {
                let mut item = DetailItem::album(&a.id, &a.title);
                if let DetailItem::Ref(ref mut r) = item {
                    r.subtitle = a.artist;
                    r.thumbnail = a.thumbnail;
                }
                item
            }
            MediaItem::Playlist(p) => {
                let mut item = DetailItem::playlist(&p.id, &p.title);
                if let DetailItem::Ref(ref mut r) = item {
                    r.subtitle = p.author;
                    r.thumbnail = p.thumbnail;
                }
                item
            }
            MediaItem::Header { title } => {
                // Headers are presentation-only - they shouldn't be in DetailItem
                // but we need a valid conversion. Create a non-navigable Ref.
                DetailItem::Ref(ContentRef {
                    id: String::new(),
                    name: title,
                    content_type: ContentType::Header,
                    thumbnail: None,
                    subtitle: None,
                })
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
    fn test_detail_item_content_ref() {
        let item = DetailItem::artist("artist123", "The Beatles");
        assert!(!item.is_header());
        assert!(item.is_focusable());
        assert!(item.is_navigable());
        assert!(!item.is_playable());
        assert_eq!(item.navigation_id(), Some("artist123"));
        assert_eq!(item.content_type(), Some(ContentType::Artist));
    }

    #[test]
    fn test_detail_item_icons() {
        let artist = DetailItem::artist("a", "Artist");
        let album = DetailItem::album("a", "Album");
        let playlist = DetailItem::playlist("p", "Playlist");

        // Icons must be actual emojis, not spaces!
        assert_eq!(artist.type_icon(), "🎤");
        assert_eq!(album.type_icon(), "💿");
        assert_eq!(playlist.type_icon(), "📁");
    }

    /// Test: From<SearchItem> correctly converts to DetailItem with proper
    /// types This is the NEW type-safe conversion (no metadata parsing)
    #[test]
    fn test_search_item_to_detail_item_artist() {
        use crate::domain::search::{ArtistItem, BrowsableItem, SearchItem};

        let artist_item = ArtistItem {
            browse_id: Some("UC123".into()),
            name: "The Beatles".into(),
            subscribers: Some("1M subscribers".into()),
            thumbnail: Some("https://example.com/thumb.jpg".into()),
        };

        let search_item = SearchItem::Browsable(BrowsableItem::Artist(artist_item));
        let detail = DetailItem::from(search_item);

        // Must be Ref with Artist type
        assert!(matches!(&detail, DetailItem::Ref(r) if r.content_type == ContentType::Artist));
        assert_eq!(detail.type_icon(), "🎤");
        assert!(detail.thumbnail_url().is_some());
    }

    /// Test: From<SearchItem> correctly converts albums
    #[test]
    fn test_search_item_to_detail_item_album() {
        use crate::domain::search::{AlbumItem, BrowsableItem, SearchItem};

        let album_item = AlbumItem {
            album_id: "MPREb123".into(),
            title: "Abbey Road".into(),
            artist: "The Beatles".into(),
            year: Some("1969".into()),
            album_type: Some("Album".into()),
            thumbnail: Some("https://example.com/cover.jpg".into()),
            explicit: false,
        };

        let search_item = SearchItem::Browsable(BrowsableItem::Album(album_item));
        let detail = DetailItem::from(search_item);

        assert!(matches!(&detail, DetailItem::Ref(r) if r.content_type == ContentType::Album));
        assert_eq!(detail.type_icon(), "💿");
        assert!(detail.thumbnail_url().is_some());
    }

    /// Test: From<Song> now simply wraps as Song (no metadata parsing)
    /// This documents the NEW behavior after removing metadata["type"] coupling
    #[test]
    fn test_song_to_detail_item_is_always_song() {
        use std::collections::HashMap;

        // Even with "type": "artist" in metadata, it's still a Song
        let mut metadata = HashMap::new();
        metadata.insert("title".into(), vec!["The Beatles".into()]);
        metadata.insert("type".into(), vec!["artist".into()]);

        let song = Song {
            id: None,
            uri: "UC123".into(),
            duration: None,
            metadata,
            last_modified: None,
            added: None,
        };

        let item = DetailItem::from(song);

        // NEW BEHAVIOR: Song is always wrapped as Song, no metadata parsing
        assert!(matches!(&item, DetailItem::Song(_)));
    }

    /// Test: All DetailItems are focusable (headers are in ListItem, not
    /// DetailItem)
    #[test]
    fn test_all_detail_items_are_focusable() {
        let artist = DetailItem::artist("a", "Artist");
        let album = DetailItem::album("a", "Album");
        let song = DetailItem::Song(Song::default());

        assert!(artist.is_focusable());
        assert!(album.is_focusable());
        assert!(song.is_focusable());
    }

    /// Test: DetailItem never reports as header
    #[test]
    fn test_detail_item_is_never_header() {
        let artist = DetailItem::artist("a", "Artist");
        let song = DetailItem::Song(Song::default());

        assert!(!artist.is_header());
        assert!(!song.is_header());
    }

    #[test]
    fn test_detail_item_header_roundtrip() {
        let header = DetailItem::header("My Header");

        assert!(header.is_header());
        assert!(!header.is_focusable());
        assert_eq!(header.primary_text(), "My Header");
    }
}
