//! ListItem - UI-only wrapper for list rendering.
//!
//! ## Purpose
//!
//! `ListItem` wraps domain items (`DetailItem`) and adds UI-only presentation
//! variants like `Header` and `Spacer`. This keeps the domain layer clean -
//! `DetailItem` contains only actionable content (Song, Ref), while `ListItem`
//! adds presentation concerns.
//!
//! ## Architecture
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
//! ```
//!
//! ## Usage
//!
//! When building a list for display:
//! ```rust,ignore
//! let items: Vec<ListItem> = vec![
//!     ListItem::Header("Top Songs".into()),
//!     ListItem::from(DetailItem::Song(song1)),
//!     ListItem::from(DetailItem::Song(song2)),
//!     ListItem::Spacer,
//!     ListItem::Header("Albums".into()),
//!     ListItem::from(DetailItem::Ref(album_ref)),
//! ];
//! ```
//!
//! When extracting for Intent (action system):
//! ```rust,ignore
//! let actionable: Vec<DetailItem> = items
//!     .iter()
//!     .filter_map(|item| item.as_content())
//!     .cloned()
//!     .collect();
//! ```

use std::borrow::Cow;

use ratatui::style::Style;

use crate::domain::{DetailItem, Song, content::ContentRef, display::ListItemDisplay};

/// A list item for UI rendering.
///
/// Wraps `DetailItem` (domain) and adds presentation-only variants.
/// This separation ensures that `Selection` in the action system
/// can never contain non-actionable items like Headers.
#[derive(Debug, Clone)]
pub enum ListItem {
    /// Actionable content from domain layer
    Content(DetailItem),

    /// Section header (non-focusable, for visual grouping)
    Header(String),

    /// Visual spacer between sections
    Spacer,
}

impl ListItem {
    /// Create a header item.
    pub fn header(title: impl Into<String>) -> Self {
        Self::Header(title.into())
    }

    /// Create a spacer item.
    pub fn spacer() -> Self {
        Self::Spacer
    }

    /// Create a content item from DetailItem.
    pub fn content(item: DetailItem) -> Self {
        Self::Content(item)
    }

    /// Check if this is a header.
    pub fn is_header(&self) -> bool {
        matches!(self, Self::Header(_))
    }

    /// Check if this is a spacer.
    pub fn is_spacer(&self) -> bool {
        matches!(self, Self::Spacer)
    }

    /// Check if this is actionable content.
    pub fn is_content(&self) -> bool {
        matches!(self, Self::Content(_))
    }

    /// Check if this item can receive focus during navigation.
    pub fn is_focusable(&self) -> bool {
        matches!(self, Self::Content(_))
    }

    /// Get the inner DetailItem if this is content.
    pub fn as_content(&self) -> Option<&DetailItem> {
        match self {
            Self::Content(item) => Some(item),
            _ => None,
        }
    }

    /// Consume and get the inner DetailItem if this is content.
    pub fn into_content(self) -> Option<DetailItem> {
        match self {
            Self::Content(item) => Some(item),
            _ => None,
        }
    }

    /// Get as song reference (convenience).
    pub fn as_song(&self) -> Option<&Song> {
        self.as_content().and_then(|c| c.as_song())
    }

    /// Get as content reference (convenience).
    pub fn as_content_ref(&self) -> Option<&ContentRef> {
        self.as_content().and_then(|c| c.as_content_ref())
    }
}

// =============================================================================
// CONVERSIONS
// =============================================================================

impl From<DetailItem> for ListItem {
    fn from(item: DetailItem) -> Self {
        Self::Content(item)
    }
}

impl From<Song> for ListItem {
    fn from(song: Song) -> Self {
        Self::Content(DetailItem::Song(song))
    }
}

impl From<ContentRef> for ListItem {
    fn from(content_ref: ContentRef) -> Self {
        Self::Content(DetailItem::Ref(content_ref))
    }
}

// =============================================================================
// LIST ITEM DISPLAY IMPLEMENTATION
// =============================================================================

impl ListItemDisplay for ListItem {
    fn primary_text(&self) -> Cow<'_, str> {
        match self {
            Self::Header(title) => Cow::Borrowed(title.as_str()),
            Self::Spacer => Cow::Borrowed(""),
            Self::Content(item) => item.primary_text(),
        }
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Content(item) => item.secondary_text(),
            _ => None,
        }
    }

    fn thumbnail_url(&self) -> Option<&str> {
        match self {
            Self::Content(item) => item.thumbnail_url(),
            _ => None,
        }
    }

    fn type_icon(&self) -> &str {
        match self {
            Self::Content(item) => item.type_icon(),
            _ => "",
        }
    }

    fn icon_style(&self) -> Style {
        match self {
            Self::Content(item) => item.icon_style(),
            _ => Style::default(),
        }
    }

    fn duration_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Content(item) => item.duration_text(),
            _ => None,
        }
    }

    fn is_playing(&self) -> bool {
        match self {
            Self::Content(item) => item.is_playing(),
            _ => false,
        }
    }

    fn is_header(&self) -> bool {
        matches!(self, Self::Header(_))
    }

    fn is_focusable(&self) -> bool {
        matches!(self, Self::Content(_))
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_item_header() {
        let item = ListItem::header("Top Songs");
        assert!(item.is_header());
        assert!(!item.is_focusable());
        assert!(item.as_content().is_none());
    }

    #[test]
    fn test_list_item_content() {
        let song = Song::default();
        let item = ListItem::from(song);
        assert!(item.is_content());
        assert!(item.is_focusable());
        assert!(item.as_content().is_some());
    }

    #[test]
    fn test_list_item_spacer() {
        let item = ListItem::spacer();
        assert!(item.is_spacer());
        assert!(!item.is_focusable());
    }

    #[test]
    fn test_into_content() {
        let song = Song::default();
        let item = ListItem::from(song.clone());
        let detail = item.into_content();
        assert!(detail.is_some());

        let header = ListItem::header("Test");
        assert!(header.into_content().is_none());
    }

    // ============================================================================
    // TDD Bug Regression Tests
    // These tests prove bugs exist (should FAIL before fix)
    // ============================================================================

    /// BUG: task-39 - All items render as songs (type distinction lost)
    ///
    /// EXPECTED BEHAVIOR: Artist/Album/Playlist refs should have distinct icons
    /// ACTUAL BEHAVIOR: All items show song icon (or no icon distinction)
    ///
    /// ROOT CAUSE: ListItem delegates to DetailItem which delegates to
    /// ContentRef This test verifies the icon chain works correctly
    #[test]
    fn list_item_from_artist_ref_has_artist_icon() {
        use crate::domain::{
            content::{ContentRef, ContentType},
            display::ListItemDisplay,
        };

        let artist_ref = ContentRef {
            content_type: ContentType::Artist,
            id: "artist123".into(),
            name: "Test Artist".into(),
            thumbnail: None,
            subtitle: None,
        };
        let detail_item = DetailItem::Ref(artist_ref);
        let list_item = ListItem::from(detail_item);

        let icon = list_item.type_icon();

        // This test verifies that artist refs get the correct icon
        // The icon should be artist-specific (👤 or Nerd Font  )
        // NOT the song icon (🎵 or empty)
        assert!(
            !icon.is_empty(),
            "BUG: Artist ref has empty icon. Expected artist icon like 👤 or  "
        );
        assert_ne!(icon, "🎵", "BUG: Artist ref has song icon. Should have artist icon.");
    }

    #[test]
    fn list_item_from_album_ref_has_album_icon() {
        use crate::domain::{
            content::{ContentRef, ContentType},
            display::ListItemDisplay,
        };

        let album_ref = ContentRef {
            content_type: ContentType::Album,
            id: "album456".into(),
            name: "Test Album".into(),
            thumbnail: None,
            subtitle: None,
        };
        let detail_item = DetailItem::Ref(album_ref);
        let list_item = ListItem::from(detail_item);

        let icon = list_item.type_icon();

        assert!(
            !icon.is_empty(),
            "BUG: Album ref has empty icon. Expected album icon like 💿 or  "
        );
        assert_ne!(icon, "🎵", "BUG: Album ref has song icon. Should have album icon.");
    }

    #[test]
    fn list_item_from_playlist_ref_has_playlist_icon() {
        use crate::domain::{
            content::{ContentRef, ContentType},
            display::ListItemDisplay,
        };

        let playlist_ref = ContentRef {
            content_type: ContentType::Playlist,
            id: "playlist789".into(),
            name: "Test Playlist".into(),
            thumbnail: None,
            subtitle: None,
        };
        let detail_item = DetailItem::Ref(playlist_ref);
        let list_item = ListItem::from(detail_item);

        let icon = list_item.type_icon();

        assert!(
            !icon.is_empty(),
            "BUG: Playlist ref has empty icon. Expected playlist icon like 📁 or  "
        );
        assert_ne!(icon, "🎵", "BUG: Playlist ref has song icon. Should have playlist icon.");
    }

    // =========================================================================
    // Task-53: Thumbnail Preservation Through UI Pipeline
    // =========================================================================

    #[test]
    fn list_item_preserves_song_thumbnail_through_full_pipeline() {
        use std::collections::HashMap;

        use crate::domain::display::ListItemDisplay;

        // Arrange: Create a Song with thumbnail in metadata
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), vec!["Test Song".to_string()]);
        metadata.insert("artist".to_string(), vec!["Test Artist".to_string()]);
        metadata.insert("thumbnail".to_string(), vec!["https://example.com/thumb.jpg".to_string()]);

        let song = Song {
            id: None,
            uri: "video123".to_string(),
            duration: None,
            metadata,
            last_modified: None,
            added: None,
        };

        // Verify Song has thumbnail
        assert_eq!(
            song.thumbnail_url(),
            Some("https://example.com/thumb.jpg"),
            "Precondition: Song should have thumbnail in metadata"
        );

        // Act: Convert Song → DetailItem → ListItem
        let detail_item = DetailItem::from(song);

        // Verify DetailItem has thumbnail
        assert_eq!(
            detail_item.thumbnail_url(),
            Some("https://example.com/thumb.jpg"),
            "BUG: DetailItem should preserve Song's thumbnail"
        );

        let list_item = ListItem::from(detail_item);

        // Assert: ListItem should have thumbnail
        assert_eq!(
            list_item.thumbnail_url(),
            Some("https://example.com/thumb.jpg"),
            "BUG: ListItem should preserve thumbnail through full Song → DetailItem → ListItem pipeline"
        );
    }

    #[test]
    fn list_item_preserves_artist_ref_thumbnail_through_full_pipeline() {
        use crate::domain::{
            content::{ContentRef, ContentType},
            display::ListItemDisplay,
        };

        // Arrange: Create a ContentRef (artist) with thumbnail
        let artist_ref = ContentRef {
            content_type: ContentType::Artist,
            id: "artist123".into(),
            name: "Famous Artist".into(),
            thumbnail: Some("https://example.com/artist.jpg".into()),
            subtitle: Some("1M subscribers".into()),
        };

        // Act: Convert to DetailItem → ListItem
        let detail_item = DetailItem::Ref(artist_ref);

        // Verify DetailItem has thumbnail
        assert_eq!(
            detail_item.thumbnail_url(),
            Some("https://example.com/artist.jpg"),
            "BUG: DetailItem::Ref should preserve ContentRef thumbnail"
        );

        let list_item = ListItem::from(detail_item);

        // Assert: ListItem should have thumbnail
        assert_eq!(
            list_item.thumbnail_url(),
            Some("https://example.com/artist.jpg"),
            "BUG: ListItem should preserve thumbnail for ContentRef through full pipeline"
        );
    }
}
