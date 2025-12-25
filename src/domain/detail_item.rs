//! DetailItem - Unified item type for navigation stacks.
//!
//! This enum represents any item that can appear in a navigation list:
//! - Songs (playable)
//! - ContentRefs (navigable - albums, artists, playlists)
//! - Headers (non-focusable section dividers)
//!
//! By using a single enum type, we eliminate the "stringly typed" pattern
//! where everything was forced into Song structs with metadata maps.
//!
//! # Design Decisions
//!
//! 1. **Type-safe navigation**: No more `metadata.get("type") == "album"` checks
//! 2. **Single stack type**: `NavStack<DetailItem>` works for search results AND detail views
//! 3. **Clean ListItemDisplay impl**: Each variant renders appropriately
//! 4. **Extensible**: Easy to add Video, Podcast, etc. in the future
//!
//! # Note on flatten_content
//!
//! The `flatten_content()` function that converts `ContentDetails` → `Vec<DetailItem>`
//! lives in `ui/widgets/detail_stack.rs` as it's a view-layer concern.

use std::borrow::Cow;
use ratatui::style::{Color, Style};

use super::display::ListItemDisplay;
use super::song::Song;
use super::content::{ContentRef, ContentType};

/// A unified item type for navigation lists.
///
/// This replaces the "stringly typed" pattern where Songs were overloaded
/// with metadata to represent albums, artists, and headers.
#[derive(Debug, Clone)]
pub enum DetailItem {
    /// A section header (non-focusable, for visual grouping)
    Header {
        title: String,
    },
    /// A playable song
    Song(Song),
    /// A navigable reference to content (album, artist, playlist)
    Ref(ContentRef),
}

impl DetailItem {
    /// Create a header item.
    pub fn header(title: impl Into<String>) -> Self {
        Self::Header { title: title.into() }
    }

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

    /// Check if this is a header.
    pub fn is_header(&self) -> bool {
        matches!(self, Self::Header { .. })
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

    /// Check if this item can receive focus during navigation.
    pub fn is_focusable(&self) -> bool {
        !self.is_header()
    }

    /// Get the ID for navigation (if applicable).
    pub fn navigation_id(&self) -> Option<&str> {
        match self {
            Self::Ref(r) => Some(&r.id),
            Self::Song(s) => Some(&s.uri),
            Self::Header { .. } => None,
        }
    }

    /// Get the content type for navigation (if applicable).
    pub fn content_type(&self) -> Option<ContentType> {
        match self {
            Self::Ref(r) => Some(r.content_type),
            Self::Song(_) => Some(ContentType::Track),
            Self::Header { .. } => None,
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
            Self::Header { title } => Cow::Borrowed(title.as_str()),
            Self::Song(song) => song.primary_text(),
            Self::Ref(r) => Cow::Borrowed(&r.name),
        }
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Header { .. } => None,
            Self::Song(song) => song.secondary_text(),
            Self::Ref(r) => r.subtitle.as_ref().map(|s| Cow::Borrowed(s.as_str())),
        }
    }

    fn thumbnail_url(&self) -> Option<&str> {
        match self {
            Self::Header { .. } => None,
            Self::Song(song) => song.thumbnail_url(),
            Self::Ref(r) => r.thumbnail.as_deref(),
        }
    }

    fn type_icon(&self) -> &str {
        match self {
            Self::Header { .. } => "",
            Self::Song(song) => song.type_icon(),
            Self::Ref(r) => match r.content_type {
                ContentType::Artist => " ",  // Nerd Font artist icon
                ContentType::Album => " ",   // Nerd Font disc icon
                ContentType::Playlist => " ", // Nerd Font list icon
                ContentType::Directory => " ", // Nerd Font folder icon
                ContentType::Video => " ",   // Nerd Font video icon
                ContentType::Track => " ",   // Nerd Font music icon
            },
        }
    }

    fn icon_style(&self) -> Style {
        match self {
            Self::Header { .. } => Style::default(),
            Self::Song(song) => song.icon_style(),
            Self::Ref(r) => match r.content_type {
                ContentType::Artist => Style::default().fg(Color::Cyan),
                ContentType::Album => Style::default().fg(Color::Magenta),
                ContentType::Playlist => Style::default().fg(Color::Green),
                ContentType::Directory => Style::default().fg(Color::Blue),
                ContentType::Video => Style::default().fg(Color::Red),
                ContentType::Track => Style::default().fg(Color::Yellow),
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
        matches!(self, Self::Header { .. })
    }

    fn is_focusable(&self) -> bool {
        !ListItemDisplay::is_header(self)
    }
}

// =============================================================================
// CONVERSION FROM SONG (for search results)
// =============================================================================

impl From<Song> for DetailItem {
    fn from(song: Song) -> Self {
        // Check if this "song" is actually a reference to another content type
        // This handles the legacy pattern where albums/artists were encoded as songs
        if let Some(item_type) = song.item_type() {
            match item_type {
                "artist" => {
                    let id = song.browse_id().unwrap_or(&song.uri).to_string();
                    let name = song.title().to_string();
                    let thumbnail = song.thumbnail_url().map(String::from);
                    return DetailItem::Ref(ContentRef {
                        id,
                        name,
                        content_type: ContentType::Artist,
                        thumbnail,
                        subtitle: None,
                    });
                }
                "album" => {
                    let id = song.browse_id().unwrap_or(&song.uri).to_string();
                    let name = song.title().to_string();
                    let thumbnail = song.thumbnail_url().map(String::from);
                    let subtitle = song.artist().map(String::from);
                    return DetailItem::Ref(ContentRef {
                        id,
                        name,
                        content_type: ContentType::Album,
                        thumbnail,
                        subtitle,
                    });
                }
                "playlist" => {
                    let id = song.browse_id().unwrap_or(&song.uri).to_string();
                    let name = song.title().to_string();
                    let thumbnail = song.thumbnail_url().map(String::from);
                    let subtitle = song.metadata.get("subtitle").and_then(|v| v.first()).cloned();
                    return DetailItem::Ref(ContentRef {
                        id,
                        name,
                        content_type: ContentType::Playlist,
                        thumbnail,
                        subtitle,
                    });
                }
                "header" => {
                    let title = song.title().to_string();
                    return DetailItem::header(title);
                }
                "video" => {
                    let id = song.browse_id().unwrap_or(&song.uri).to_string();
                    let name = song.title().to_string();
                    let thumbnail = song.thumbnail_url().map(String::from);
                    let subtitle = song.artist().map(String::from);
                    return DetailItem::Ref(ContentRef {
                        id,
                        name,
                        content_type: ContentType::Video,
                        thumbnail,
                        subtitle,
                    });
                }
                _ => {} // Fall through to Song
            }
        }

        DetailItem::Song(song)
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detail_item_header() {
        let item = DetailItem::header("Top Songs");
        assert!(item.is_header());
        assert!(!item.is_focusable());
        assert!(!item.is_navigable());
        assert!(!item.is_playable());
        assert_eq!(item.primary_text(), "Top Songs");
    }

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

        assert_eq!(artist.type_icon(), " ");
        assert_eq!(album.type_icon(), " ");
        assert_eq!(playlist.type_icon(), " ");
    }
}
