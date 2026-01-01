//! ArtistDetailPane - DetailPane implementation for artist content.
//!
//! Displays artist information with:
//! - Top songs section
//! - Albums section
//! - Singles section
//! - Related artists section
//!
//! Uses ContentView<ArtistContent> for unified stack and key handling.

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    ctx::Ctx,
    domain::{ArtistContent, DetailItem},
    domain::content::ContentType,
    shared::key_event::KeyEvent,
    ui::panes::navigator_types::{
        ContentAction, DetailId, DetailPane, EntityContent, EntityRef, InputMode, NavigatorPane, PaneAction, PaneId,
    },
    ui::widgets::content_view::ContentView,
};

// =============================================================================
// ARTIST DETAIL PANE
// =============================================================================

/// DetailPane for displaying artist content with stacking.
///
/// Uses ContentView<ArtistContent> for all stack management and key handling.
/// Supports navigating through multiple artists (e.g., Artist A → Related Artist B).
#[derive(Debug, Clone, Default)]
pub struct ArtistDetailPane {
    /// ContentView handles all stack management and key handling
    view: ContentView<ArtistContent>,
}

impl ArtistDetailPane {
    /// Create a new empty ArtistDetailPane.
    pub fn new() -> Self {
        Self { view: ContentView::new() }
    }
}

// =============================================================================
// PANE TRAIT IMPLEMENTATION
// =============================================================================

impl NavigatorPane for ArtistDetailPane {
    fn id(&self) -> PaneId {
        PaneId::Detail(DetailId::Artist)
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
            ContentAction::Activate(item) => self.resolve_action(item),
            ContentAction::Mark(_) => PaneAction::Handled,
            ContentAction::MoveUp(_) | ContentAction::MoveDown(_) | ContentAction::Delete(_) => {
                PaneAction::Handled
            }
            ContentAction::Enqueue(items) => {
                // 'a' key: Add to queue without playing
                let songs: Vec<_> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .cloned()
                    .collect();
                if !songs.is_empty() {
                    PaneAction::Enqueue(songs)
                } else {
                    PaneAction::Handled
                }
            }
        })
    }
}

impl ArtistDetailPane {
    /// Interpret what activation means for a DetailItem in ArtistDetailPane.
    ///
    /// Uses Selection to handle marked-items-vs-current logic uniformly.
    fn resolve_action(&self, item: DetailItem) -> PaneAction {
        match item {
            DetailItem::Song(song) => {
                // Use Selection to get marked items or fall back to current
                let selection = self.view.get_selection();
                let songs = selection.songs_cloned();

                if songs.len() > 1 {
                    // Multiple songs selected - play all starting from activated song
                    let start_index = selection
                        .find_song_index(&song.uri)
                        .unwrap_or(0);
                    PaneAction::PlayAll { songs, start_index }
                } else {
                    // Single song - play it
                    PaneAction::Play(song)
                }
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

impl DetailPane for ArtistDetailPane {
    fn detail_id(&self) -> DetailId {
        DetailId::Artist
    }

    fn has_content(&self) -> bool {
        self.view.has_content()
    }

    fn stack_depth(&self) -> usize {
        self.view.stack_depth()
    }

    fn push(&mut self, content: EntityContent) {
        if let EntityContent::Artist(artist) = content {
            self.view.push(artist);
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

    fn make_test_artist() -> ArtistContent {
        ArtistContent {
            id: "artist123".to_string(),
            name: "Test Artist".to_string(),
            top_songs: vec![],
            thumbnail: None,
            bio: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn test_artist_pane_creation() {
        let pane = ArtistDetailPane::new();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_artist_pane_push_pop() {
        let mut pane = ArtistDetailPane::new();

        // Push first artist
        pane.push(EntityContent::Artist(make_test_artist()));
        assert!(pane.has_content());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Artist"));

        // Push second artist
        let mut artist2 = make_test_artist();
        artist2.name = "Artist 2".to_string();
        pane.push(EntityContent::Artist(artist2));
        assert_eq!(pane.stack_depth(), 2);
        assert_eq!(pane.current_title(), Some("Artist 2"));

        // Pop should go back to first
        assert!(pane.pop());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Artist"));

        // Pop at depth 1 should fail
        assert!(!pane.pop());
        assert_eq!(pane.stack_depth(), 1);
    }

    #[test]
    fn test_artist_pane_clear() {
        let mut pane = ArtistDetailPane::new();
        pane.push(EntityContent::Artist(make_test_artist()));
        pane.push(EntityContent::Artist(make_test_artist()));

        pane.clear();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_pane_id() {
        let pane = ArtistDetailPane::new();
        assert_eq!(pane.id(), PaneId::Detail(DetailId::Artist));
        assert_eq!(pane.detail_id(), DetailId::Artist);
    }
}
