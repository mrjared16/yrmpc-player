//! AlbumDetailPane - DetailPane implementation for album content.
//!
//! Displays album information with:
//! - Album tracks section
//! - Related albums section (if available)
//!
//! Uses ContentView<AlbumContent> for unified stack and key handling.

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    ctx::Ctx,
    domain::AlbumContent,
    shared::key_event::KeyEvent,
    ui::panes::navigator_types::{
        DetailId, DetailPane, EntityContent, InputMode, NavigatorPane, PaneAction, PaneId,
    },
    ui::widgets::content_view::ContentView,
};

// =============================================================================
// ALBUM DETAIL PANE
// =============================================================================

/// DetailPane for displaying album content with stacking.
///
/// Uses ContentView<AlbumContent> for all stack management and key handling.
#[derive(Debug, Clone, Default)]
pub struct AlbumDetailPane {
    /// ContentView handles all stack management and key handling
    view: ContentView<AlbumContent>,
}

impl AlbumDetailPane {
    /// Create a new empty AlbumDetailPane.
    pub fn new() -> Self {
        Self { view: ContentView::new() }
    }
}

// =============================================================================
// PANE TRAIT IMPLEMENTATION
// =============================================================================

impl NavigatorPane for AlbumDetailPane {
    fn id(&self) -> PaneId {
        PaneId::Detail(DetailId::Album)
    }

    fn mode(&self) -> InputMode {
        self.view.mode()
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        self.view.render(frame, area, ctx);
        Ok(())
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        Ok(self.view.handle_key(key, ctx).into())
    }
}

// =============================================================================
// DETAIL PANE TRAIT IMPLEMENTATION
// =============================================================================

impl DetailPane for AlbumDetailPane {
    fn detail_id(&self) -> DetailId {
        DetailId::Album
    }

    fn has_content(&self) -> bool {
        self.view.has_content()
    }

    fn stack_depth(&self) -> usize {
        self.view.stack_depth()
    }

    fn push(&mut self, content: EntityContent) {
        if let EntityContent::Album(album) = content {
            self.view.push(album);
        }
    }

    fn pop(&mut self) -> bool {
        self.view.pop()
    }

    fn clear(&mut self) {
        self.view.clear();
    }

    fn current_title(&self) -> Option<&str> {
        self.view.current_title()
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::content::{ContentRef, Extensions};

    fn make_test_album() -> AlbumContent {
        AlbumContent {
            id: "album123".to_string(),
            title: "Test Album".to_string(),
            artist: ContentRef::artist("a1", "Test Artist"),
            tracks: vec![],
            thumbnail: None,
            year: None,
            release_type: None,
            description: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn test_album_pane_creation() {
        let pane = AlbumDetailPane::new();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_album_pane_push_pop() {
        let mut pane = AlbumDetailPane::new();

        // Push first album
        pane.push(EntityContent::Album(make_test_album()));
        assert!(pane.has_content());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Album"));

        // Push second album
        let mut album2 = make_test_album();
        album2.title = "Album 2".to_string();
        pane.push(EntityContent::Album(album2));
        assert_eq!(pane.stack_depth(), 2);
        assert_eq!(pane.current_title(), Some("Album 2"));

        // Pop should go back to first
        assert!(pane.pop());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Album"));

        // Pop at depth 1 should fail
        assert!(!pane.pop());
        assert_eq!(pane.stack_depth(), 1);
    }

    #[test]
    fn test_album_pane_clear() {
        let mut pane = AlbumDetailPane::new();
        pane.push(EntityContent::Album(make_test_album()));
        pane.push(EntityContent::Album(make_test_album()));

        pane.clear();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_pane_id() {
        let pane = AlbumDetailPane::new();
        assert_eq!(pane.id(), PaneId::Detail(DetailId::Album));
        assert_eq!(pane.detail_id(), DetailId::Album);
    }
}
