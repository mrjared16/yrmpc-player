//! SectionList - Sectioned list component for detail panes.
//!
//! Provides a flat-navigable list with preserved section structure for rendering.
//! Used by DetailPanes (Artist, Album, Playlist) to display hierarchical content.
//!
//! ## Design (SOLID - SRP)
//!
//! - **Flat navigation**: j/k moves through all items across sections
//! - **Sectioned rendering**: Each section can have different layout (list/grid)
//! - **Section navigation**: Tab/Shift-Tab jumps between sections
//! - **Delegates to InteractiveListView**: For list state and key handling
//! - **Translates ListAction → SectionAction**: Adding item context
//!
//! ## Layered Architecture
//!
//! ```text
//! SectionList.handle_key()
//!   ↓ delegates to
//! InteractiveListView.handle_key() → ListAction
//!   ↓ translates to
//! SectionAction (with DetailItem context)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let sections = build_sections(&content);
//! let mut list = SectionList::new(sections);
//!
//! match list.handle_key(key, ctx) {
//!     SectionAction::Activate(item) => { /* navigate or play */ }
//!     SectionAction::Delete(items) => { /* remove from queue */ }
//!     SectionAction::Back => { /* return to navigator */ }
//!     _ => {}
//! }
//! ```

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    widgets::{Block, Borders},
    Frame,
};

use crate::ctx::Ctx;
use crate::domain::DetailItem;
use crate::shared::key_event::KeyEvent;
use crate::ui::panes::navigator_types::{InputMode, ListAction, SectionAction, EscResult, BackspaceResult};
use crate::ui::widgets::detail_stack::SectionView;
use crate::ui::widgets::interactive_list_view::{InteractiveListView, NavConfig};

// =============================================================================
// SECTION LIST
// =============================================================================

/// A navigable list with preserved section structure.
///
/// Wraps `Vec<SectionView>` with an `InteractiveListView` for unified navigation.
/// Sections are preserved for layout but navigation is flat.
#[derive(Debug, Clone)]
pub struct SectionList {
    /// Sections with layout hints
    sections: Vec<SectionView>,
    /// Flattened items for navigation (cached)
    flat_items: Vec<DetailItem>,
    /// Interactive list view for navigation
    list_view: InteractiveListView,
    /// Title for display
    title: String,
}

impl Default for SectionList {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl SectionList {
    /// Create a new SectionList from sections.
    pub fn new(sections: Vec<SectionView>) -> Self {
        let flat_items: Vec<DetailItem> = sections
            .iter()
            .flat_map(|s| s.items.iter().cloned())
            .collect();

        let mut list_view = InteractiveListView::new();

        // Select first focusable item
        if let Some(first) = flat_items.iter().position(|item| item.is_focusable()) {
            list_view.select(Some(first));
        }

        Self {
            sections,
            flat_items,
            list_view,
            title: String::new(),
        }
    }

    /// Create with a title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Replace sections (rebuilds flat items).
    pub fn set_sections(&mut self, sections: Vec<SectionView>) {
        self.flat_items = sections
            .iter()
            .flat_map(|s| s.items.iter().cloned())
            .collect();
        self.sections = sections;

        // Reset selection to first focusable
        if let Some(first) = self.flat_items.iter().position(|item| item.is_focusable()) {
            self.list_view.select(Some(first));
        } else {
            self.list_view.select(None);
        }
    }

    // =========================================================================
    // ACCESSORS
    // =========================================================================

    /// Get sections reference.
    pub fn sections(&self) -> &[SectionView] {
        &self.sections
    }

    /// Get flat items reference.
    pub fn items(&self) -> &[DetailItem] {
        &self.flat_items
    }

    /// Get item count.
    pub fn len(&self) -> usize {
        self.flat_items.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.flat_items.is_empty()
    }

    /// Get selected index.
    pub fn selected(&self) -> Option<usize> {
        self.list_view.selected()
    }

    /// Get selected item.
    pub fn selected_item(&self) -> Option<&DetailItem> {
        self.list_view.selected().and_then(|idx| self.flat_items.get(idx))
    }

    /// Get current input mode.
    pub fn mode(&self) -> InputMode {
        self.list_view.mode()
    }

    /// Access underlying list view.
    pub fn list_view(&self) -> &InteractiveListView {
        &self.list_view
    }

    /// Access underlying list view mutably.
    pub fn list_view_mut(&mut self) -> &mut InteractiveListView {
        &mut self.list_view
    }

    // =========================================================================
    // NAVIGATION
    // =========================================================================

    /// Move selection down.
    pub fn select_next(&mut self) {
        self.list_view.select_next(&self.flat_items, NavConfig::default());
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        self.list_view.select_prev(&self.flat_items, NavConfig::default());
    }

    /// Jump to first item.
    pub fn select_first(&mut self) {
        self.list_view.select_first(&self.flat_items);
    }

    /// Jump to last item.
    pub fn select_last(&mut self) {
        self.list_view.select_last(&self.flat_items);
    }

    /// Page down (half viewport).
    pub fn page_down(&mut self) {
        self.list_view.next_half_viewport(&self.flat_items, NavConfig::default());
    }

    /// Page up (half viewport).
    pub fn page_up(&mut self) {
        self.list_view.prev_half_viewport(&self.flat_items, NavConfig::default());
    }

    // =========================================================================
    // FIND MODE
    // =========================================================================

    /// Enter find mode with optional initial text.
    pub fn enter_find_mode(&mut self, initial: &str) {
        self.list_view.enter_find_mode(&self.flat_items, initial);
    }

    /// Handle Esc key (mode-aware).
    pub fn handle_esc(&mut self) -> EscResult {
        self.list_view.handle_esc()
    }

    /// Handle Backspace key (mode-aware).
    pub fn handle_backspace(&mut self) -> BackspaceResult {
        self.list_view.handle_backspace(&self.flat_items)
    }

    /// Handle character in find mode.
    pub fn handle_find_char(&mut self, ch: char) -> bool {
        self.list_view.handle_find_char(&self.flat_items, ch)
    }

    /// Jump to next find match.
    pub fn find_next(&mut self) {
        self.list_view.filter_next_match();
    }

    /// Jump to previous find match.
    pub fn find_prev(&mut self) {
        self.list_view.filter_prev_match();
    }

    // =========================================================================
    // MARKS
    // =========================================================================

    /// Toggle mark on selected item.
    pub fn toggle_mark(&mut self) {
        self.list_view.toggle_mark();
    }

    /// Get marked indices.
    pub fn marked_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.list_view.marked_indices()
    }

    /// Check if any items are marked.
    pub fn has_marked(&self) -> bool {
        self.list_view.has_marked()
    }

    /// Clear all marks.
    pub fn clear_marks(&mut self) {
        self.list_view.clear_marks();
    }

    /// Get marked items.
    pub fn marked_items(&self) -> Vec<&DetailItem> {
        self.list_view
            .marked_indices()
            .filter_map(|idx| self.flat_items.get(idx))
            .collect()
    }

    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the section list.
    ///
    /// Currently renders as a flat list. Future: render per-section with layout hints.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;

        // Build title with counts
        let title = if self.title.is_empty() {
            format!(" ({}) ", self.flat_items.len())
        } else if self.list_view.has_marked() {
            format!(
                " {} ({}) [{} marked] ",
                self.title,
                self.flat_items.len(),
                self.list_view.marked().len()
            )
        } else {
            format!(" {} ({}) ", self.title, self.flat_items.len())
        };

        // Add find indicator if active
        let title = if let Some(filter_display) = self.list_view.filter_display() {
            format!("{} {} ", title.trim_end(), filter_display)
        } else {
            title
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(config.as_border_style());

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Update viewport height
        self.list_view.set_viewport_height(inner.height);

        // Render items using InteractiveListView
        // TODO: In future, render per-section with different layouts
        self.list_view.render_simple(frame, inner, ctx, &self.flat_items, None);
    }

    // =========================================================================
    // UNIFIED KEY HANDLING
    // =========================================================================

    /// Unified key handling for sectioned lists.
    ///
    /// Delegates to InteractiveListView for list operations, handles
    /// section-specific keys (Tab/Shift-Tab), and translates ListAction
    /// to SectionAction with item context.
    ///
    /// ## Layered Architecture
    ///
    /// ```text
    /// SectionList.handle_key()
    ///   ↓ handles Tab/Shift-Tab (section navigation)
    ///   ↓ delegates other keys to
    /// InteractiveListView.handle_key() → ListAction
    ///   ↓ translates to
    /// SectionAction (with DetailItem context)
    /// ```
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> SectionAction {
        // Edit mode: passthrough to pane
        if self.mode() == InputMode::Edit {
            return SectionAction::Passthrough;
        }

        // Section navigation (SectionList's unique responsibility)
        if self.mode() == InputMode::Normal {
            match key.code() {
                KeyCode::Tab => {
                    self.next_section();
                    key.stop_propagation();
                    return SectionAction::Handled;
                }
                KeyCode::BackTab => {
                    self.prev_section();
                    key.stop_propagation();
                    return SectionAction::Handled;
                }
                _ => {}
            }
        }

        // Delegate to InteractiveListView
        let list_action = self.list_view.handle_key(key, &self.flat_items, ctx);

        // Translate ListAction to SectionAction
        self.translate_list_action(list_action)
    }

    /// Translate ListAction to SectionAction by adding item context.
    fn translate_list_action(&self, action: ListAction) -> SectionAction {
        match action {
            ListAction::Handled => SectionAction::Handled,

            ListAction::Activate(idx) => {
                if let Some(item) = self.flat_items.get(idx) {
                    SectionAction::Activate(item.clone())
                } else {
                    SectionAction::Handled
                }
            }

            ListAction::Mark(indices) => {
                let items = self.indices_to_items(&indices);
                SectionAction::Mark(items)
            }

            ListAction::MoveUp(indices) => {
                let items = self.indices_to_items(&indices);
                SectionAction::MoveUp(items)
            }

            ListAction::MoveDown(indices) => {
                let items = self.indices_to_items(&indices);
                SectionAction::MoveDown(items)
            }

            ListAction::Delete(indices) => {
                let items = self.indices_to_items(&indices);
                SectionAction::Delete(items)
            }

            ListAction::Back => SectionAction::Back,

            ListAction::Passthrough => SectionAction::Passthrough,
        }
    }

    /// Convert indices to DetailItems.
    fn indices_to_items(&self, indices: &[usize]) -> Vec<DetailItem> {
        indices
            .iter()
            .filter_map(|&idx| self.flat_items.get(idx).cloned())
            .collect()
    }

    // =========================================================================
    // SECTION NAVIGATION
    // =========================================================================

    /// Jump to first focusable item of next section.
    pub fn next_section(&mut self) {
        let current = self.selected().unwrap_or(0);

        // Find which section we're in and where the next one starts
        let mut item_offset = 0;
        for (i, section) in self.sections.iter().enumerate() {
            let section_end = item_offset + section.items.len();

            if current < section_end {
                // Current item is in this section
                // Jump to start of next section
                if i + 1 < self.sections.len() {
                    let next_start = section_end;
                    // Find first focusable in next section
                    if let Some(idx) = self.flat_items[next_start..]
                        .iter()
                        .position(|item| item.is_focusable())
                    {
                        self.list_view.select(Some(next_start + idx));
                    }
                }
                return;
            }

            item_offset = section_end;
        }
    }

    /// Jump to first focusable item of previous section.
    pub fn prev_section(&mut self) {
        let current = self.selected().unwrap_or(0);

        let mut item_offset = 0;
        let mut prev_section_start = 0;

        for section in self.sections.iter() {
            let section_end = item_offset + section.items.len();

            if current < section_end && current >= item_offset {
                // Current item is in this section
                if item_offset > 0 {
                    // Jump to first focusable in previous section
                    if let Some(idx) = self.flat_items[prev_section_start..item_offset]
                        .iter()
                        .position(|item| item.is_focusable())
                    {
                        self.list_view.select(Some(prev_section_start + idx));
                    }
                }
                return;
            }

            prev_section_start = item_offset;
            item_offset = section_end;
        }
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::content::SectionKey;
    use crate::ui::panes::navigator_types::EscResult;

    fn make_test_sections() -> Vec<SectionView> {
        vec![
            SectionView::new(
                SectionKey::Stats,
                "Section 1",
                vec![
                    DetailItem::header("Header 1"),
                    DetailItem::artist("a1", "Artist 1"),
                    DetailItem::artist("a2", "Artist 2"),
                ],
            ),
            SectionView::new(
                SectionKey::Albums,
                "Section 2",
                vec![
                    DetailItem::album("al1", "Album 1"),
                ],
            ),
        ]
    }

    #[test]
    fn test_section_list_creation() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        assert_eq!(list.sections().len(), 2);
        assert_eq!(list.items().len(), 4);
        // First focusable should be selected (index 1, skipping header)
        assert_eq!(list.selected(), Some(1));
    }

    #[test]
    fn test_section_list_navigation() {
        let sections = make_test_sections();
        let mut list = SectionList::new(sections);

        // Start at first focusable (index 1)
        assert_eq!(list.selected(), Some(1));

        // Move down
        list.select_next();
        assert_eq!(list.selected(), Some(2));

        // Move down again (crosses section boundary)
        list.select_next();
        assert_eq!(list.selected(), Some(3));

        // Move up
        list.select_prev();
        assert_eq!(list.selected(), Some(2));
    }

    #[test]
    fn test_section_list_esc_handling() {
        let sections = make_test_sections();
        let mut list = SectionList::new(sections);

        // In Normal mode with no filter, Esc should return BackPane
        let result = list.list_view.handle_esc();
        assert_eq!(result, EscResult::BackPane);

        // Enter find mode
        list.enter_find_mode("");
        assert_eq!(list.mode(), InputMode::Find);

        // Esc should exit find mode
        let result = list.list_view.handle_esc();
        assert_eq!(result, EscResult::Handled);
        assert_eq!(list.mode(), InputMode::Normal);
    }

    #[test]
    fn test_section_list_marks() {
        let sections = make_test_sections();
        let mut list = SectionList::new(sections);

        assert!(!list.has_marked());

        list.toggle_mark();
        assert!(list.has_marked());

        let marked: Vec<_> = list.marked_indices().collect();
        assert_eq!(marked, vec![1]); // First focusable was selected

        list.clear_marks();
        assert!(!list.has_marked());
    }

    #[test]
    fn test_translate_list_action_activate() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Test Activate translation
        let action = list.translate_list_action(ListAction::Activate(1));
        match action {
            SectionAction::Activate(item) => {
                assert!(matches!(item, DetailItem::Ref(_)));
            }
            _ => panic!("Expected SectionAction::Activate"),
        }
    }

    #[test]
    fn test_translate_list_action_delete() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Test Delete translation
        let action = list.translate_list_action(ListAction::Delete(vec![1, 2]));
        match action {
            SectionAction::Delete(items) => {
                assert_eq!(items.len(), 2);
            }
            _ => panic!("Expected SectionAction::Delete"),
        }
    }
}
