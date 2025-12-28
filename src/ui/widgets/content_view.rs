//! ContentView - Unified content stacking and key handling component.
//!
//! Per ARCHITECTURE.md, ContentView is the single component used by ALL panes
//! that display content. It replaces the previous NavStack and manual stacks.
//!
//! ## Design
//!
//! - Generic over `C: ContentViewable` trait (defined in domain::content)
//! - Owns a stack of `ContentLevel<C>` (content + SectionList)
//! - Provides unified `handle_key()` → `ContentAction`
//! - Delegates key handling to SectionList
//! - Translates SectionAction to ContentAction
//!
//! ## Usage
//!
//! ```ignore
//! // In a DetailPane
//! struct ArtistDetailPane {
//!     view: ContentView<ArtistContent>,
//! }
//!
//! impl NavigatorPane for ArtistDetailPane {
//!     fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
//!         Ok(self.view.handle_key(key, ctx).into())
//!     }
//! }
//! ```

use ratatui::{Frame, prelude::Rect};

use crate::ctx::Ctx;
use crate::domain::ContentViewable;
use crate::shared::key_event::KeyEvent;
use crate::ui::widgets::detail_stack::build_sections;
use crate::ui::widgets::section_list::SectionList;

// Re-export ContentAction from navigator_types for backwards compatibility
pub use crate::ui::panes::navigator_types::{ContentAction, InputMode, SectionAction};

// =============================================================================
// CONTENT LEVEL
// =============================================================================

/// A single level in the content stack.
///
/// Each level contains the original content and a SectionList for navigation.
#[derive(Debug, Clone)]
pub struct ContentLevel<C> {
    /// Original content (preserved for actions/refresh)
    pub content: C,
    /// Section list for navigation and rendering
    pub section_list: SectionList,
}

impl<C: ContentViewable> ContentLevel<C> {
    /// Create a new content level from content.
    pub fn new(content: C) -> Self {
        let title = content.title().to_string();
        let details = content.to_content_details();
        let sections = build_sections(&details);
        let section_list = SectionList::new(sections).with_title(title);

        Self {
            content,
            section_list,
        }
    }
}

// =============================================================================
// CONTENT VIEW
// =============================================================================

/// Unified content stacking and key handling component.
///
/// Generic over content type `C`. Provides:
/// - Stack management (push/pop/clear)
/// - Unified key handling via SectionList
/// - Action translation (SectionAction → ContentAction)
/// - Rendering delegation to SectionList
#[derive(Debug, Clone, Default)]
pub struct ContentView<C: ContentViewable> {
    /// Stack of content levels (index 0 is root, last is current)
    stack: Vec<ContentLevel<C>>,
}

impl<C: ContentViewable> ContentView<C> {
    /// Create an empty ContentView.
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    // =========================================================================
    // STACK OPERATIONS
    // =========================================================================

    /// Push content onto the stack.
    pub fn push(&mut self, content: C) {
        self.stack.push(ContentLevel::new(content));
    }

    /// Pop content from the stack.
    ///
    /// Returns false if at bottom (single item or empty).
    pub fn pop(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    /// Clear all content from the stack.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Check if the stack has any content.
    pub fn has_content(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Get the stack depth.
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }

    /// Check if we can pop internally (stack depth > 1).
    pub fn can_pop(&self) -> bool {
        self.stack.len() > 1
    }

    /// Get reference to current level.
    pub fn current(&self) -> Option<&ContentLevel<C>> {
        self.stack.last()
    }

    /// Get mutable reference to current level.
    pub fn current_mut(&mut self) -> Option<&mut ContentLevel<C>> {
        self.stack.last_mut()
    }

    /// Get the current content's title.
    pub fn current_title(&self) -> Option<&str> {
        self.current().map(|l| l.content.title())
    }

    /// Get breadcrumb path.
    pub fn breadcrumb(&self) -> String {
        self.stack
            .iter()
            .map(|l| l.content.title())
            .collect::<Vec<_>>()
            .join(" > ")
    }

    // =========================================================================
    // MODE
    // =========================================================================

    /// Get the current input mode.
    pub fn mode(&self) -> InputMode {
        self.current()
            .map(|l| l.section_list.mode())
            .unwrap_or(InputMode::Normal)
    }

    // =========================================================================
    // KEY HANDLING
    // =========================================================================

    /// Handle a key event.
    ///
    /// Delegates to SectionList and BUBBLES the result to pane.
    /// Per ADR: ContentView does NOT interpret actions - pane decides what Activate means.
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> ContentAction {
        let Some(level) = self.current_mut() else {
            return ContentAction::Back;
        };

        match level.section_list.handle_key(key, ctx) {
            SectionAction::Handled => ContentAction::Handled,

            // BUBBLE: Let pane decide what activation means
            SectionAction::Activate(item) => ContentAction::Activate(item),

            SectionAction::Back => {
                // Try to pop stack first
                if self.pop() {
                    ContentAction::Handled
                } else {
                    ContentAction::Back
                }
            }

            // BUBBLE: Let pane handle marked items
            SectionAction::Mark(items) => ContentAction::Mark(items),

            // BUBBLE: Let pane handle move/delete (Queue, Library, etc.)
            SectionAction::MoveUp(items) => ContentAction::MoveUp(items),
            SectionAction::MoveDown(items) => ContentAction::MoveDown(items),
            SectionAction::Delete(items) => ContentAction::Delete(items),

            SectionAction::Passthrough => ContentAction::Handled,
        }
    }

    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the current content level.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        if let Some(level) = self.current_mut() {
            level.section_list.render(frame, area, ctx);
        }
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::content::{ArtistContent, Extensions};

    fn make_test_artist(name: &str) -> ArtistContent {
        ArtistContent {
            id: format!("artist_{}", name),
            name: name.to_string(),
            top_songs: vec![],
            thumbnail: None,
            bio: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn test_content_view_creation() {
        let view: ContentView<ArtistContent> = ContentView::new();
        assert!(!view.has_content());
        assert_eq!(view.stack_depth(), 0);
    }

    #[test]
    fn test_content_view_push_pop() {
        let mut view: ContentView<ArtistContent> = ContentView::new();

        // Push first artist
        view.push(make_test_artist("Artist A"));
        assert!(view.has_content());
        assert_eq!(view.stack_depth(), 1);
        assert_eq!(view.current_title(), Some("Artist A"));

        // Push second artist
        view.push(make_test_artist("Artist B"));
        assert_eq!(view.stack_depth(), 2);
        assert_eq!(view.current_title(), Some("Artist B"));

        // Pop should go back to first
        assert!(view.pop());
        assert_eq!(view.stack_depth(), 1);
        assert_eq!(view.current_title(), Some("Artist A"));

        // Pop at depth 1 should fail
        assert!(!view.pop());
        assert_eq!(view.stack_depth(), 1);
    }

    #[test]
    fn test_content_view_clear() {
        let mut view: ContentView<ArtistContent> = ContentView::new();
        view.push(make_test_artist("Artist A"));
        view.push(make_test_artist("Artist B"));

        view.clear();
        assert!(!view.has_content());
        assert_eq!(view.stack_depth(), 0);
    }

    #[test]
    fn test_content_view_breadcrumb() {
        let mut view: ContentView<ArtistContent> = ContentView::new();
        view.push(make_test_artist("Artist A"));
        view.push(make_test_artist("Artist B"));

        assert_eq!(view.breadcrumb(), "Artist A > Artist B");
    }
}
