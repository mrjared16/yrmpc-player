//! DetailStack - Composable detail view component for ContentDetails navigation.
//!
//! Per ARCHITECTURE.md, DetailStack is a view-layer component that:
//! - Holds a stack of DetailViews for drill-down navigation
//! - Flattens ContentDetails into Vec<DetailItem> for display
//! - Reuses InteractiveListView for vim-style controls
//! - Manages breadcrumb path for display
//!
//! # Usage
//!
//! ```ignore
//! // In a pane
//! let mut detail_stack = DetailStack::new();
//!
//! // When user navigates to an artist
//! detail_stack.push(content_details, "KIMLONG");
//!
//! // Render (if active)
//! if detail_stack.is_active() {
//!     detail_stack.render(frame, area, ctx);
//! }
//!
//! // Back navigation
//! if !detail_stack.pop() {
//!     // Stack is empty, return to search results
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
// DETAIL VIEW
// =============================================================================

/// A single detail view in the stack.
///
/// Contains the original ContentDetails plus the flattened items for display.
#[derive(Debug, Clone)]
pub struct DetailView {
    /// Original content (for reference/actions)
    pub content: ContentDetails,
    /// Flattened items for list display
    pub items: Vec<DetailItem>,
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
        let items = flatten_content(&content);
        let mut view = InteractiveListView::new();
        
        // Select first focusable item
        if let Some(first_focusable) = items.iter().position(|item| item.is_focusable()) {
            view.select(Some(first_focusable));
        }
        
        Self {
            content,
            items,
            view,
            load_state: LoadState::Loaded,
            title,
        }
    }

    /// Get selected item reference.
    pub fn selected_item(&self) -> Option<&DetailItem> {
        self.view.selected().and_then(|idx| self.items.get(idx))
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
// FLATTEN CONTENT
// =============================================================================

/// Flatten ContentDetails into a Vec<DetailItem> for display.
///
/// This converts the structured content (with sections) into a flat list
/// that can be displayed in an InteractiveListView.
///
/// The order follows a consistent pattern:
/// 1. Primary tracks/songs (with header if non-empty)
/// 2. Extension sections in order (Albums, Singles, Related, etc.)
pub fn flatten_content(content: &ContentDetails) -> Vec<DetailItem> {
    let mut items = Vec::new();

    match content {
        ContentDetails::Album(album) => {
            // Album: just tracks (no header needed - album title is in the pane title)
            if !album.tracks.is_empty() {
                items.push(DetailItem::header("Tracks"));
                items.extend(album.tracks.iter().cloned().map(DetailItem::Song));
            }

            // Add extension sections
            flatten_extensions(&album.extensions, &mut items);
        }

        ContentDetails::Artist(artist) => {
            // Artist: top songs first
            if !artist.top_songs.is_empty() {
                items.push(DetailItem::header("Top Songs"));
                items.extend(artist.top_songs.iter().cloned().map(DetailItem::Song));
            }

            // Add extension sections (albums, singles, related artists, etc.)
            flatten_extensions(&artist.extensions, &mut items);
        }

        ContentDetails::Playlist(playlist) => {
            // Playlist: just tracks
            if !playlist.tracks.is_empty() {
                items.push(DetailItem::header("Tracks"));
                items.extend(playlist.tracks.iter().cloned().map(DetailItem::Song));
            }

            // Add extension sections
            flatten_extensions(&playlist.extensions, &mut items);
        }
    }

    items
}

/// Flatten extension sections into the item list.
fn flatten_extensions(extensions: &Extensions, items: &mut Vec<DetailItem>) {
    for section in extensions.iter() {
        // Skip stats and actions - they're rendered separately
        if matches!(section.key, SectionKey::Stats | SectionKey::Actions) {
            continue;
        }

        match &section.content {
            SectionData::Items(refs) if !refs.is_empty() => {
                if !section.title.is_empty() {
                    items.push(DetailItem::header(&section.title));
                }
                items.extend(refs.iter().cloned().map(DetailItem::Ref));
            }
            SectionData::Tracks(songs) if !songs.is_empty() => {
                if !section.title.is_empty() {
                    items.push(DetailItem::header(&section.title));
                }
                items.extend(songs.iter().cloned().map(DetailItem::Song));
            }
            SectionData::Paginated { items: refs, .. } if !refs.is_empty() => {
                if !section.title.is_empty() {
                    items.push(DetailItem::header(&section.title));
                }
                items.extend(refs.iter().cloned().map(DetailItem::Ref));
            }
            _ => {} // Skip empty or other section types
        }
    }
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
    fn test_flatten_empty_content() {
        let album = ContentDetails::Album(AlbumContent {
            id: "test".into(),
            title: "Test Album".into(),
            artist: ContentRef::artist("a1", "Artist"),
            tracks: vec![],
            ..Default::default()
        });

        let items = flatten_content(&album);
        assert!(items.is_empty());
    }

    #[test]
    fn test_flatten_with_extensions() {
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

        let items = flatten_content(&album);
        // Should have: Header("More by Artist"), Ref(Album 2), Ref(Album 3)
        assert_eq!(items.len(), 3);
        assert!(items[0].is_header());
        assert!(items[1].is_navigable());
        assert!(items[2].is_navigable());
    }
}
