//! InputContentView - Composition of input zone and content view.
//!
//! Used by panes that need both input (search box, filter) and list content.
//! Examples: SearchPane, LibraryPane (future).
//!
//! ## Design (SOLID - SRP + OCP)
//!
//! - **Composition**: Owns input component + ContentView
//! - **Focus management**: Tracks which zone is focused (Input/Content)
//! - **Key routing**: Routes keys to focused zone
//! - **Focus switching**: Arrow keys to switch between zones
//!
//! ## Layered Architecture
//!
//! ```text
//! InputContentView
//!   ├── Input zone (I: InputZone trait)
//!   └── Content zone (ContentView<C>)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! struct SearchPane {
//!     view: InputContentView<SearchInputGroups, SearchContent>,
//! }
//!
//! impl NavigatorPane for SearchPane {
//!     fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
//!         match self.view.handle_key(key, ctx) {
//!             InputContentAction::Search(query) => { /* execute search */ }
//!             InputContentAction::Content(action) => { /* handle content action */ }
//!             _ => {}
//!         }
//!         Ok(PaneAction::Handled)
//!     }
//! }
//! ```

use crossterm::event::KeyCode;
use ratatui::{Frame, layout::Rect};

use crate::ctx::Ctx;
use crate::domain::ContentViewable;
use crate::shared::key_event::KeyEvent;
use crate::ui::panes::navigator_types::InputMode;
use crate::ui::widgets::content_view::{ContentAction, ContentView};

// =============================================================================
// INPUT ZONE TRAIT
// =============================================================================

/// Trait for input zone components.
///
/// Implemented by any input component that can be composed with ContentView.
pub trait InputZone {
    /// Render the input zone.
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx, is_focused: bool);

    /// Handle key input when this zone is focused.
    ///
    /// Returns the action result.
    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> InputZoneAction;

    /// Get current input mode (Edit when typing, Normal otherwise).
    fn mode(&self) -> InputMode;

    /// Focus the first element in the input zone.
    fn focus_first(&mut self);

    /// Focus the last element in the input zone.
    fn focus_last(&mut self);

    /// Check if we're at the last element (for focus switching).
    fn is_at_last(&self) -> bool;

    /// Check if we're at the first element (for focus switching).
    fn is_at_first(&self) -> bool;
}

/// Action returned from InputZone key handling.
#[derive(Debug, Clone)]
pub enum InputZoneAction {
    /// Key was handled
    Handled,
    /// Submit input (e.g., press Enter on search)
    Submit(String),
    /// Move focus to content zone
    FocusContent,
    /// Key not handled
    Passthrough,
}

// =============================================================================
// FOCUS ZONE
// =============================================================================

/// Which zone is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusZone {
    #[default]
    Input,
    Content,
}

// =============================================================================
// INPUT CONTENT ACTION
// =============================================================================

/// Action returned from InputContentView.
#[derive(Debug, Clone)]
pub enum InputContentAction {
    /// Key was handled internally
    Handled,
    /// Input was submitted (e.g., search query)
    Submit(String),
    /// Content action bubbled up
    Content(ContentAction),
    /// Back requested from content
    Back,
}

// =============================================================================
// INPUT CONTENT VIEW
// =============================================================================

/// Composition of input zone and content view.
///
/// Generic over:
/// - `I`: Input zone component implementing `InputZone`
/// - `C`: Content type implementing `ContentViewable`
#[derive(Debug)]
pub struct InputContentView<I: InputZone, C: ContentViewable> {
    /// Input zone (search box, filter, etc.)
    pub input: I,
    /// Content view (list of results)
    pub content: ContentView<C>,
    /// Currently focused zone
    focus: FocusZone,
}

impl<I: InputZone, C: ContentViewable> InputContentView<I, C> {
    /// Create a new InputContentView.
    pub fn new(input: I) -> Self {
        Self {
            input,
            content: ContentView::new(),
            focus: FocusZone::Input,
        }
    }

    /// Create with existing content.
    pub fn with_content(input: I, content: ContentView<C>) -> Self {
        Self {
            input,
            content,
            focus: FocusZone::Input,
        }
    }

    // =========================================================================
    // FOCUS
    // =========================================================================

    /// Get current focus zone.
    pub fn focus(&self) -> FocusZone {
        self.focus
    }

    /// Set focus zone.
    pub fn set_focus(&mut self, zone: FocusZone) {
        self.focus = zone;
    }

    /// Focus the input zone.
    pub fn focus_input(&mut self) {
        self.focus = FocusZone::Input;
    }

    /// Focus the content zone.
    pub fn focus_content(&mut self) {
        self.focus = FocusZone::Content;
    }

    // =========================================================================
    // MODE
    // =========================================================================

    /// Get current input mode based on focused zone.
    pub fn mode(&self) -> InputMode {
        match self.focus {
            FocusZone::Input => self.input.mode(),
            FocusZone::Content => self.content.mode(),
        }
    }

    // =========================================================================
    // KEY HANDLING
    // =========================================================================

    /// Handle key input.
    ///
    /// Routes to focused zone, handles focus switching.
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> InputContentAction {
        match self.focus {
            FocusZone::Input => self.handle_input_key(key, ctx),
            FocusZone::Content => self.handle_content_key(key, ctx),
        }
    }

    /// Handle key when input zone is focused.
    fn handle_input_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> InputContentAction {
        // Check for focus switching first (only in Normal mode)
        if self.input.mode() == InputMode::Normal {
            match key.code() {
                // Down at last input -> focus content
                KeyCode::Char('j') | KeyCode::Down if self.input.is_at_last() => {
                    if self.content.has_content() {
                        self.focus = FocusZone::Content;
                        key.stop_propagation();
                        return InputContentAction::Handled;
                    }
                }
                // Tab -> focus content
                KeyCode::Tab => {
                    if self.content.has_content() {
                        self.focus = FocusZone::Content;
                        key.stop_propagation();
                        return InputContentAction::Handled;
                    }
                }
                _ => {}
            }
        }

        // Delegate to input zone
        match self.input.handle_key(key, ctx) {
            InputZoneAction::Handled => InputContentAction::Handled,
            InputZoneAction::Submit(query) => InputContentAction::Submit(query),
            InputZoneAction::FocusContent => {
                if self.content.has_content() {
                    self.focus = FocusZone::Content;
                }
                InputContentAction::Handled
            }
            InputZoneAction::Passthrough => InputContentAction::Handled,
        }
    }

    /// Handle key when content zone is focused.
    fn handle_content_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> InputContentAction {
        // Check for focus switching first
        if self.content.mode() == InputMode::Normal {
            match key.code() {
                // Shift+Tab -> focus input
                KeyCode::BackTab => {
                    self.focus = FocusZone::Input;
                    self.input.focus_last();
                    key.stop_propagation();
                    return InputContentAction::Handled;
                }
                _ => {}
            }
        }

        // Delegate to content view
        let action = self.content.handle_key(key, ctx);
        
        match &action {
            ContentAction::BackPane => {
                // Go back to input zone first
                self.focus = FocusZone::Input;
                InputContentAction::Handled
            }
            ContentAction::BackStage => {
                // Go back to input zone
                self.focus = FocusZone::Input;
                InputContentAction::Handled
            }
            _ => InputContentAction::Content(action),
        }
    }

    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the input content view.
    ///
    /// Caller is responsible for layout (splitting area for input vs content).
    pub fn render_input(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let is_focused = self.focus == FocusZone::Input;
        self.input.render(frame, area, ctx, is_focused);
    }

    /// Render the content zone.
    pub fn render_content(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        self.content.render(frame, area, ctx);
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Mock input zone for testing
    #[derive(Debug, Default)]
    struct MockInputZone {
        mode: InputMode,
        at_first: bool,
        at_last: bool,
    }

    impl InputZone for MockInputZone {
        fn render(&mut self, _frame: &mut Frame, _area: Rect, _ctx: &Ctx, _is_focused: bool) {}

        fn handle_key(&mut self, _key: &mut KeyEvent, _ctx: &Ctx) -> InputZoneAction {
            InputZoneAction::Handled
        }

        fn mode(&self) -> InputMode {
            self.mode
        }

        fn focus_first(&mut self) {
            self.at_first = true;
            self.at_last = false;
        }

        fn focus_last(&mut self) {
            self.at_first = false;
            self.at_last = true;
        }

        fn is_at_last(&self) -> bool {
            self.at_last
        }

        fn is_at_first(&self) -> bool {
            self.at_first
        }
    }

    #[test]
    fn test_focus_zone_default() {
        assert_eq!(FocusZone::default(), FocusZone::Input);
    }

    #[test]
    fn test_input_content_view_creation() {
        use crate::domain::content::ArtistContent;
        
        let input = MockInputZone::default();
        let view: InputContentView<_, ArtistContent> = InputContentView::new(input);
        
        assert_eq!(view.focus(), FocusZone::Input);
        assert!(!view.content.has_content());
    }

    #[test]
    fn test_focus_switching() {
        use crate::domain::content::ArtistContent;
        
        let input = MockInputZone::default();
        let mut view: InputContentView<_, ArtistContent> = InputContentView::new(input);
        
        view.focus_content();
        assert_eq!(view.focus(), FocusZone::Content);
        
        view.focus_input();
        assert_eq!(view.focus(), FocusZone::Input);
    }
}
