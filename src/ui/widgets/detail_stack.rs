//! DetailStack - Composable detail view component for ContentDetails navigation.
//!
//! Per ARCHITECTURE.md, DetailStack uses a **sectioned design**:
//! - `DetailView` holds `Vec<SectionView>` (structure preserved)
//! - Navigation is flat (one `InteractiveListView`)
//! - Rendering can vary per section (list now, grid future)
//!
//! ## Why Sectioned?
//!
//! The original `flatten_content()` approach lost structure, making it impossible
//! to render different sections with different layouts (e.g., albums as grid).
//!
//! The sectioned design:
//! - Preserves structure for flexible rendering
//! - Keeps navigation simple (flat iteration)
//! - Enables future presets/grid without redesign
//!
//! ## Usage
//!
//! ```ignore
//! let view = DetailView::new(content_details);
//! 
//! // Navigation: flat iteration
//! for item in view.items() { ... }
//!
//! // Rendering: per-section layout
//! for section in &view.sections {
//!     render_header(&section.title);
//!     match section.layout {
//!         LayoutKind::List => render_list(&section.items),
//!         LayoutKind::Grid { columns } => render_grid(&section.items, columns),
//!     }
//! }
//! ```

use crate::domain::{
    ContentDetails, DetailItem, Song,
    content::{Extensions, SectionData, SectionKey},
};
use super::interactive_list_view::InteractiveListView;

// =============================================================================
// LOAD STATE
// =============================================================================

/// Loading state for async content fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadState {
    #[default]
    Idle,
    Loading,
    Loaded,
    Error,
}

// =============================================================================
// LAYOUT KIND
// =============================================================================

/// Layout hint for section rendering.
///
/// Currently only List is rendered; Grid is defined for future use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutKind {
    #[default]
    List,
    /// Grid layout with specified column count (future)
    Grid { columns: u8 },
}

// =============================================================================
// SECTION VIEW
// =============================================================================

/// A section of content with layout hint.
///
/// Sections preserve the structure of `ContentDetails` while enabling
/// per-section rendering (list vs grid) in the future.
#[derive(Debug, Clone)]
pub struct SectionView {
    /// Section identifier for preset config lookup
    pub key: SectionKey,
    /// Header text (empty = no header)
    pub title: String,
    /// Layout hint for rendering
    pub layout: LayoutKind,
    /// Items in this section
    pub items: Vec<DetailItem>,
}

impl SectionView {
    /// Create a new section.
    pub fn new(key: SectionKey, title: impl Into<String>, items: Vec<DetailItem>) -> Self {
        Self {
            key,
            title: title.into(),
            layout: LayoutKind::default(),
            items,
        }
    }

    /// Set layout hint.
    pub fn with_layout(mut self, layout: LayoutKind) -> Self {
        self.layout = layout;
        self
    }

    /// Check if section is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// =============================================================================
// DETAIL VIEW
// =============================================================================

/// A single detail view with sectioned structure.
///
/// Contains the original `ContentDetails` plus sections for rendering.
/// Navigation uses flat iteration via `items()`.
#[derive(Debug, Clone)]
pub struct DetailView {
    /// Original content (preserved for actions/refresh)
    pub content: ContentDetails,
    /// Sections for structured rendering
    pub sections: Vec<SectionView>,
    /// View state (selection, marks, filter)
    pub view: InteractiveListView,
    /// Loading state
    pub load_state: LoadState,
    /// Title for breadcrumb
    pub title: String,
}

impl DetailView {
    /// Create a new DetailView from ContentDetails.
    pub fn new(content: ContentDetails) -> Self {
        let title = content.title().to_string();
        let sections = build_sections(&content);
        let mut view = InteractiveListView::new();
        
        // Select first focusable item
        let items: Vec<_> = sections.iter().flat_map(|s| s.items.iter()).collect();
        if let Some(first_focusable) = items.iter().position(|item| item.is_focusable()) {
            view.select(Some(first_focusable));
        }
        
        Self {
            content,
            sections,
            view,
            load_state: LoadState::Loaded,
            title,
        }
    }

    /// Iterate all items as flat list (for navigation).
    pub fn items(&self) -> impl Iterator<Item = &DetailItem> {
        self.sections.iter().flat_map(|s| s.items.iter())
    }

    /// Total item count across all sections.
    pub fn item_count(&self) -> usize {
        self.sections.iter().map(|s| s.items.len()).sum()
    }

    /// Get item at global index.
    pub fn item_at(&self, index: usize) -> Option<&DetailItem> {
        self.items().nth(index)
    }

    /// Get selected item reference.
    pub fn selected_item(&self) -> Option<&DetailItem> {
        self.view.selected().and_then(|idx| self.item_at(idx))
    }

    /// Check if sections is empty.
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|s| s.is_empty())
    }
}

// =============================================================================
// DETAIL STACK
// =============================================================================

/// Stack-based detail view navigation.
///
/// Manages a stack of DetailViews for drill-down navigation through
/// artists → albums → tracks.
#[derive(Debug, Clone, Default)]
pub struct DetailStack {
    /// Stack of views (index 0 is root)
    views: Vec<DetailView>,
}

impl DetailStack {
    /// Create an empty stack.
    pub fn new() -> Self {
        Self { views: Vec::new() }
    }

    /// Check if stack is active (has views).
    pub fn is_active(&self) -> bool {
        !self.views.is_empty()
    }

    /// Get current depth.
    pub fn depth(&self) -> usize {
        self.views.len()
    }

    /// Push a new detail view onto the stack.
    pub fn push(&mut self, content: ContentDetails) {
        self.views.push(DetailView::new(content));
    }

    /// Pop the top view, returns false if stack is empty after pop.
    pub fn pop(&mut self) -> bool {
        self.views.pop();
        !self.views.is_empty()
    }

    /// Clear the stack.
    pub fn clear(&mut self) {
        self.views.clear();
    }

    /// Get current (top) view.
    pub fn current(&self) -> Option<&DetailView> {
        self.views.last()
    }

    /// Get current view mutable.
    pub fn current_mut(&mut self) -> Option<&mut DetailView> {
        self.views.last_mut()
    }

    /// Get breadcrumb path.
    pub fn breadcrumb(&self) -> String {
        self.views
            .iter()
            .map(|v| v.title.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    }

    /// Get breadcrumb segments.
    pub fn breadcrumb_segments(&self) -> Vec<&str> {
        self.views.iter().map(|v| v.title.as_str()).collect()
    }
}

// =============================================================================
// BUILD SECTIONS
// =============================================================================

/// Build sections from ContentDetails, preserving structure.
///
/// This replaces `flatten_content()`. Instead of losing structure,
/// sections are kept separate for per-section layout in the future.
pub fn build_sections(content: &ContentDetails) -> Vec<SectionView> {
    let mut sections = Vec::new();

    match content {
        ContentDetails::Album(album) => {
            // Album: tracks section
            if !album.tracks.is_empty() {
                sections.push(SectionView::new(
                    SectionKey::Stats, // Using Stats as placeholder for "Tracks"
                    "Tracks",
                    album.tracks.iter().cloned().map(DetailItem::Song).collect(),
                ));
            }
            // Add extension sections
            build_extension_sections(&album.extensions, &mut sections);
        }

        ContentDetails::Artist(artist) => {
            // Artist: top songs section
            if !artist.top_songs.is_empty() {
                sections.push(SectionView::new(
                    SectionKey::Stats, // Using Stats as placeholder for "Top Songs"
                    "Top Songs",
                    artist.top_songs.iter().cloned().map(DetailItem::Song).collect(),
                ));
            }
            // Add extension sections (albums, singles, related)
            build_extension_sections(&artist.extensions, &mut sections);
        }

        ContentDetails::Playlist(playlist) => {
            // Playlist: tracks section
            if !playlist.tracks.is_empty() {
                sections.push(SectionView::new(
                    SectionKey::Stats, // Using Stats as placeholder for "Tracks"
                    "Tracks",
                    playlist.tracks.iter().cloned().map(DetailItem::Song).collect(),
                ));
            }
            // Add extension sections
            build_extension_sections(&playlist.extensions, &mut sections);
        }
    }

    sections
}

/// Build sections from Extensions container.
fn build_extension_sections(extensions: &Extensions, sections: &mut Vec<SectionView>) {
    for section in extensions.iter() {
        // Skip stats and actions - they're rendered separately (not in list)
        if matches!(section.key, SectionKey::Stats | SectionKey::Actions) {
            continue;
        }

        let items: Vec<DetailItem> = match &section.content {
            SectionData::Items(refs) if !refs.is_empty() => {
                refs.iter().cloned().map(DetailItem::Ref).collect()
            }
            SectionData::Tracks(songs) if !songs.is_empty() => {
                songs.iter().cloned().map(DetailItem::Song).collect()
            }
            SectionData::Paginated { items: refs, .. } if !refs.is_empty() => {
                refs.iter().cloned().map(DetailItem::Ref).collect()
            }
            _ => continue, // Skip empty sections
        };

        // Determine default layout based on section type
        let layout = match section.key {
            // Albums/artists could be grid in the future
            SectionKey::Albums | SectionKey::Singles | SectionKey::RelatedArtists => {
                LayoutKind::List // For now, will be Grid { columns: 3 } in future
            }
            _ => LayoutKind::List,
        };

        sections.push(
            SectionView::new(section.key, &section.title, items)
                .with_layout(layout)
        );
    }
}

// =============================================================================
// COMPATIBILITY HELPERS
// =============================================================================

/// Convert sections to flat items with headers (for NavStack compatibility).
///
/// This is a bridge function for SearchPaneV2 which still uses NavStack<DetailItem>.
/// In the future, SearchPaneV2 should use DetailStack directly.
pub fn sections_to_items(sections: &[SectionView]) -> Vec<DetailItem> {
    let mut items = Vec::new();
    for section in sections {
        if !section.title.is_empty() && !section.items.is_empty() {
            items.push(DetailItem::header(&section.title));
        }
        items.extend(section.items.iter().cloned());
    }
    items
}

/// Build sections and flatten to items (convenience function).
///
/// Equivalent to: `sections_to_items(&build_sections(content))`
pub fn flatten_content(content: &ContentDetails) -> Vec<DetailItem> {
    sections_to_items(&build_sections(content))
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::content::{AlbumContent, ArtistContent, ContentRef, Extensions};

    #[test]
    fn test_detail_stack_lifecycle() {
        let mut stack = DetailStack::new();
        assert!(!stack.is_active());
        assert_eq!(stack.depth(), 0);

        // Push an album
        let album = ContentDetails::Album(AlbumContent {
            id: "album1".into(),
            title: "Abbey Road".into(),
            artist: ContentRef::artist("a1", "The Beatles"),
            tracks: vec![],
            ..Default::default()
        });
        stack.push(album);

        assert!(stack.is_active());
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.breadcrumb(), "Abbey Road");

        // Push an artist
        let artist = ContentDetails::Artist(ArtistContent {
            id: "artist1".into(),
            name: "KIMLONG".into(),
            top_songs: vec![],
            ..Default::default()
        });
        stack.push(artist);

        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.breadcrumb(), "Abbey Road > KIMLONG");

        // Pop
        assert!(stack.pop());
        assert_eq!(stack.depth(), 1);

        // Pop again - now empty
        assert!(!stack.pop());
        assert!(!stack.is_active());
    }

    #[test]
    fn test_build_sections_empty_content() {
        let album = ContentDetails::Album(AlbumContent {
            id: "test".into(),
            title: "Test Album".into(),
            artist: ContentRef::artist("a1", "Artist"),
            tracks: vec![],
            ..Default::default()
        });

        let sections = build_sections(&album);
        assert!(sections.is_empty());
    }

    #[test]
    fn test_build_sections_with_extensions() {
        let album = ContentDetails::Album(AlbumContent {
            id: "test".into(),
            title: "Test Album".into(),
            artist: ContentRef::artist("a1", "Artist"),
            tracks: vec![],
            extensions: Extensions::builder()
                .related_albums("More by Artist", vec![
                    ContentRef::album("a2", "Album 2"),
                    ContentRef::album("a3", "Album 3"),
                ])
                .build(),
            ..Default::default()
        });

        let sections = build_sections(&album);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "More by Artist");
        assert_eq!(sections[0].items.len(), 2);
    }

    #[test]
    fn test_detail_view_flat_iteration() {
        let artist = ContentDetails::Artist(ArtistContent {
            id: "a1".into(),
            name: "Test Artist".into(),
            top_songs: vec![],
            extensions: Extensions::builder()
                .albums("Albums", vec![
                    ContentRef::album("alb1", "Album 1"),
                    ContentRef::album("alb2", "Album 2"),
                ])
                .singles("Singles", vec![
                    ContentRef::album("sin1", "Single 1"),
                ])
                .build(),
            ..Default::default()
        });

        let view = DetailView::new(artist);
        
        // Should have 2 sections
        assert_eq!(view.sections.len(), 2);
        
        // Flat iteration should yield 3 items total
        assert_eq!(view.item_count(), 3);
        assert_eq!(view.items().count(), 3);
    }
}
