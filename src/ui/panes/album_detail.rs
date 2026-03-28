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
    backends::youtube::protocol::play_intent::{ContextSource, PlayIntent},
    ctx::Ctx,
    domain::{AlbumContent, DetailItem, content::ContentType},
    shared::key_event::KeyEvent,
    ui::{
        panes::navigator_types::{
            ContentAction, DetailId, DetailPane, EntityContent, EntityRef, InputMode,
            NavigatorPane, PaneAction, PaneId,
        },
        widgets::content_view::ContentView,
    },
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
        Ok(match self.view.handle_key(key, ctx) {
            ContentAction::Handled => PaneAction::Handled,
            ContentAction::Back => PaneAction::BackPane,
            ContentAction::Activate(item) => self.resolve_action(item, ctx),
            ContentAction::Mark(_) => PaneAction::Handled,
            ContentAction::MoveUp(_) | ContentAction::MoveDown(_) | ContentAction::Delete(_) => {
                PaneAction::Handled // Not applicable in detail panes
            }
            ContentAction::Enqueue(items) => {
                // 'a' key: Add to queue without playing
                let songs: Vec<_> = items.iter().filter_map(|i| i.as_song()).cloned().collect();
                if !songs.is_empty() { PaneAction::Enqueue(songs) } else { PaneAction::Handled }
            }
            ContentAction::Passthrough => PaneAction::Handled,
        })
    }
}

impl AlbumDetailPane {
    /// Interpret what activation means for a DetailItem in AlbumDetailPane.
    ///
    /// Uses Selection to handle marked-items-vs-current logic uniformly.
    fn resolve_action(&self, item: DetailItem, ctx: &Ctx) -> PaneAction {
        match item {
            DetailItem::Song(song) => {
                // Use Selection to get marked items or fall back to current
                let selection = self.view.get_selection();
                let songs = selection.songs_cloned();

                // Get the album ID from current content
                let album_id =
                    self.view.current().map(|level| level.content.id.clone()).unwrap_or_default();

                if songs.len() > 1 {
                    // Multiple songs selected - play all starting from activated song
                    let start_index = selection.find_song_index(&song.uri).unwrap_or(0);
                    ctx.queue_store().play(PlayIntent::Context {
                        tracks: songs,
                        offset: start_index,
                        shuffle: false,
                        source: Some(ContextSource::Album { album_id }),
                    });
                } else {
                    // Single song - play it
                    ctx.queue_store().play(PlayIntent::Context {
                        tracks: vec![song],
                        offset: 0,
                        shuffle: false,
                        source: Some(ContextSource::Album { album_id }),
                    });
                }
                PaneAction::Handled
            }
            DetailItem::Ref(content_ref) => {
                let entity_type = match content_ref.content_type {
                    ContentType::Artist => DetailId::Artist,
                    ContentType::Album => DetailId::Album,
                    ContentType::Playlist => DetailId::Playlist,
                    _ => return PaneAction::Handled,
                };
                PaneAction::NavigateTo(EntityRef {
                    entity_type,
                    id: content_ref.id,
                    name: content_ref.name,
                })
            }
        }
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
