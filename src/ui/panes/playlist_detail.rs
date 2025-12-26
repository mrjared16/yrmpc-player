//! PlaylistDetailPane - DetailPane implementation for playlist content.
//!
//! Displays playlist information with:
//! - Playlist tracks section
//! - Related playlists section (if available)
//!
//! Uses ContentView<PlaylistContent> for unified stack and key handling.

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    ctx::Ctx,
    domain::PlaylistContent,
    shared::key_event::KeyEvent,
    ui::panes::navigator_types::{
        DetailId, DetailPane, EntityContent, InputMode, NavigatorPane, PaneAction, PaneId,
    },
    ui::widgets::content_view::ContentView,
};

// =============================================================================
// PLAYLIST DETAIL PANE
// =============================================================================

/// DetailPane for displaying playlist content with stacking.
///
/// Uses ContentView<PlaylistContent> for all stack management and key handling.
#[derive(Debug, Clone, Default)]
pub struct PlaylistDetailPane {
    /// ContentView handles all stack management and key handling
    view: ContentView<PlaylistContent>,
}

impl PlaylistDetailPane {
    /// Create a new empty PlaylistDetailPane.
    pub fn new() -> Self {
        Self { view: ContentView::new() }
    }
}

// =============================================================================
// PANE TRAIT IMPLEMENTATION
// =============================================================================

impl NavigatorPane for PlaylistDetailPane {
    fn id(&self) -> PaneId {
        PaneId::Detail(DetailId::Playlist)
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

impl DetailPane for PlaylistDetailPane {
    fn detail_id(&self) -> DetailId {
        DetailId::Playlist
    }

    fn has_content(&self) -> bool {
        self.view.has_content()
    }

    fn stack_depth(&self) -> usize {
        self.view.stack_depth()
    }

    fn push(&mut self, content: EntityContent) {
        if let EntityContent::Playlist(playlist) = content {
            self.view.push(playlist);
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
    use crate::domain::content::Extensions;

    fn make_test_playlist() -> PlaylistContent {
        PlaylistContent {
            id: "playlist123".to_string(),
            title: "Test Playlist".to_string(),
            tracks: vec![],
            author: None,
            thumbnail: None,
            description: None,
            track_count: None,
            duration_text: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn test_playlist_pane_creation() {
        let pane = PlaylistDetailPane::new();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_playlist_pane_push_pop() {
        let mut pane = PlaylistDetailPane::new();

        // Push first playlist
        pane.push(EntityContent::Playlist(make_test_playlist()));
        assert!(pane.has_content());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Playlist"));

        // Push second playlist
        let mut playlist2 = make_test_playlist();
        playlist2.title = "Playlist 2".to_string();
        pane.push(EntityContent::Playlist(playlist2));
        assert_eq!(pane.stack_depth(), 2);
        assert_eq!(pane.current_title(), Some("Playlist 2"));

        // Pop should go back to first
        assert!(pane.pop());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Playlist"));

        // Pop at depth 1 should fail
        assert!(!pane.pop());
        assert_eq!(pane.stack_depth(), 1);
    }

    #[test]
    fn test_playlist_pane_clear() {
        let mut pane = PlaylistDetailPane::new();
        pane.push(EntityContent::Playlist(make_test_playlist()));
        pane.push(EntityContent::Playlist(make_test_playlist()));

        pane.clear();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_pane_id() {
        let pane = PlaylistDetailPane::new();
        assert_eq!(pane.id(), PaneId::Detail(DetailId::Playlist));
        assert_eq!(pane.detail_id(), DetailId::Playlist);
    }
}
