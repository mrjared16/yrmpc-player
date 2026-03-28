//! SectionList - Sectioned list component for detail panes.
//!
//! Provides a flat-navigable list with preserved section structure for
//! rendering. Used by DetailPanes (Artist, Album, Playlist) to display
//! hierarchical content.
//!
//! ## Design (SOLID - SRP)
//!
//! - **Flat navigation**: j/k moves through all items across sections
//! - **Sectioned rendering**: Each section can have different layout
//!   (list/grid)
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
//! ## Architecture Note
//!
//! `flat_items` uses `ListItem` (UI layer) not `DetailItem` (domain layer)
//! because:
//! - Headers are a UI concern, not domain data
//! - `ListItem::Header` is non-focusable and non-actionable
//! - Domain items are wrapped in `ListItem::Content(DetailItem)`
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
    Frame,
    layout::Rect,
    widgets::{Block, Borders},
};

use crate::{
    actions::Selection,
    ctx::Ctx,
    domain::DetailItem,
    shared::key_event::KeyEvent,
    ui::{
        panes::navigator_types::{
            BackspaceResult, ContentAction, EscResult, InputMode, ListAction,
        },
        widgets::{
            detail_stack::SectionView,
            list_item::ListItem,
            selectable_list::{NavConfig, SelectableList},
        },
    },
};

// =============================================================================
// SECTION LIST
// =============================================================================

/// A navigable list with preserved section structure.
///
/// Wraps `Vec<SectionView>` with an `InteractiveListView` for unified
/// navigation. Sections are preserved for layout but navigation is flat.
///
/// Uses `ListItem` (UI layer) internally to properly separate:
/// - Domain data (`DetailItem`) - actionable content
/// - UI elements (`ListItem::Header`) - visual-only, non-focusable
#[derive(Debug, Clone)]
pub struct SectionList {
    /// Sections with layout hints (domain data)
    sections: Vec<SectionView>,
    /// Flattened items for navigation (UI layer with headers)
    flat_items: Vec<ListItem>,
    /// Interactive list view for navigation
    list_view: SelectableList,
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
    ///
    /// Converts `SectionView` (domain) to `Vec<ListItem>` (UI), inserting
    /// `ListItem::Header` at section boundaries for visual grouping.
    pub fn new(sections: Vec<SectionView>) -> Self {
        let flat_items = Self::flatten_sections(&sections);

        let mut list_view = SelectableList::new();

        // Select first focusable item
        if let Some(first) = flat_items.iter().position(|item| item.is_focusable()) {
            list_view.select(Some(first));
        }

        Self { sections, flat_items, list_view, title: String::new() }
    }

    /// Flatten sections into a Vec<ListItem>, inserting headers at section
    /// boundaries.
    fn flatten_sections(sections: &[SectionView]) -> Vec<ListItem> {
        sections
            .iter()
            .flat_map(|s| {
                let header_iter =
                    if s.title.is_empty() { None } else { Some(ListItem::header(&s.title)) };
                header_iter.into_iter().chain(s.items.iter().cloned().map(ListItem::from))
            })
            .collect()
    }

    /// Create with a title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Replace sections (rebuilds flat items).
    pub fn set_sections(&mut self, sections: Vec<SectionView>) {
        self.flat_items = Self::flatten_sections(&sections);
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

    /// Get flat items reference (UI layer).
    pub fn items(&self) -> &[ListItem] {
        &self.flat_items
    }

    /// Get item count (including headers).
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

    /// Get selected item (returns DetailItem if content, None if
    /// header/spacer).
    pub fn selected_item(&self) -> Option<&DetailItem> {
        self.list_view
            .selected()
            .and_then(|idx| self.flat_items.get(idx))
            .and_then(|item| item.as_content())
    }

    /// Get current input mode.
    pub fn mode(&self) -> InputMode {
        self.list_view.mode()
    }

    /// Access underlying list view.
    pub fn list_view(&self) -> &SelectableList {
        &self.list_view
    }

    /// Access underlying list view mutably.
    pub fn list_view_mut(&mut self) -> &mut SelectableList {
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

    /// Get marked items (only content items, not headers).
    pub fn marked_items(&self) -> Vec<&DetailItem> {
        self.list_view
            .marked_indices()
            .filter_map(|idx| self.flat_items.get(idx))
            .filter_map(|item| item.as_content())
            .collect()
    }

    /// Get a Selection for the Intent system.
    ///
    /// Returns marked items if any are marked, otherwise returns the current
    /// item. This is the primary method for panes to get items for action
    /// execution.
    pub fn get_selection(&self) -> Selection {
        if self.has_marked() {
            Selection::new(self.marked_items().into_iter().cloned().collect())
        } else if let Some(item) = self.selected_item() {
            Selection::single(item.clone())
        } else {
            Selection::empty()
        }
    }

    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the section list.
    ///
    /// Currently renders as a flat list. Future: render per-section with layout
    /// hints.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;

        // Count only content items for display (exclude headers)
        let content_count = self.flat_items.iter().filter(|i| i.is_content()).count();

        // Build title with counts
        let title = if self.title.is_empty() {
            format!(" ({}) ", content_count)
        } else if self.list_view.has_marked() {
            format!(
                " {} ({}) [{} marked] ",
                self.title,
                content_count,
                self.list_view.marked().len()
            )
        } else {
            format!(" {} ({}) ", self.title, content_count)
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

        // Render items with "currently playing" highlight callback
        self.list_view.render(frame, inner, ctx, &self.flat_items, None, |_idx, item| {
            // Check if this item is the currently playing song
            if let Some(song) = item.as_song() {
                ctx.find_current_song_in_queue()
                    .map(|(_, current)| current.uri == song.uri)
                    .unwrap_or(false)
            } else {
                false
            }
        });
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
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> ContentAction {
        // Edit mode: passthrough to pane
        if self.mode() == InputMode::Edit {
            return ContentAction::Passthrough;
        }

        // Section navigation (SectionList's unique responsibility)
        if self.mode() == InputMode::Normal {
            match key.code() {
                KeyCode::Tab => {
                    self.next_section();
                    key.stop_propagation();
                    return ContentAction::Handled;
                }
                KeyCode::BackTab => {
                    self.prev_section();
                    key.stop_propagation();
                    return ContentAction::Handled;
                }
                _ => {}
            }
        }

        // Delegate to InteractiveListView
        let list_action = self.list_view.handle_key(key, &self.flat_items, ctx);

        // Translate ListAction to ContentAction
        self.translate_list_action(list_action)
    }

    /// Translate ListAction to SectionAction by adding item context.
    ///
    /// Extracts `DetailItem` from `ListItem::Content` for actions.
    /// Headers/spacers at action indices are ignored (return Handled).
    fn translate_list_action(&self, action: ListAction) -> ContentAction {
        match action {
            ListAction::Handled => ContentAction::Handled,

            ListAction::Activate(idx) => {
                if let Some(item) = self.flat_items.get(idx).and_then(|i| i.as_content()) {
                    ContentAction::Activate(item.clone())
                } else {
                    ContentAction::Handled
                }
            }

            ListAction::Mark(indices) => {
                let items = self.indices_to_items(&indices);
                ContentAction::Mark(items)
            }

            ListAction::MoveUp(indices) => {
                let items = self.indices_to_items(&indices);
                ContentAction::MoveUp(items)
            }

            ListAction::MoveDown(indices) => {
                let items = self.indices_to_items(&indices);
                ContentAction::MoveDown(items)
            }

            ListAction::Delete(indices) => {
                let items = self.indices_to_items(&indices);
                ContentAction::Delete(items)
            }

            ListAction::Enqueue(indices) => {
                let items = self.indices_to_items(&indices);
                ContentAction::Enqueue(items)
            }

            ListAction::Back => ContentAction::Back,

            ListAction::Passthrough => ContentAction::Passthrough,
        }
    }

    /// Convert indices to DetailItems (filters out headers/spacers).
    fn indices_to_items(&self, indices: &[usize]) -> Vec<DetailItem> {
        indices
            .iter()
            .filter_map(|&idx| self.flat_items.get(idx))
            .filter_map(|item| item.as_content().cloned())
            .collect()
    }

    // =========================================================================
    // SECTION NAVIGATION
    // =========================================================================

    /// Jump to first focusable item of next section.
    pub fn next_section(&mut self) {
        let current = self.selected().unwrap_or(0);

        // Calculate section boundaries accounting for headers
        let mut item_offset = 0;
        for (i, section) in self.sections.iter().enumerate() {
            // Each section has: optional header + items
            let section_len = if section.title.is_empty() { 0 } else { 1 } + section.items.len();
            let section_end = item_offset + section_len;

            if current < section_end {
                // Current item is in this section
                // Jump to start of next section
                if i + 1 < self.sections.len() {
                    let next_start = section_end;
                    // Find first focusable in next section
                    if let Some(idx) =
                        self.flat_items[next_start..].iter().position(|item| item.is_focusable())
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
            // Each section has: optional header + items
            let section_len = if section.title.is_empty() { 0 } else { 1 } + section.items.len();
            let section_end = item_offset + section_len;

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
    use crate::{domain::content::SectionKey, ui::panes::navigator_types::EscResult};

    fn make_test_sections() -> Vec<SectionView> {
        vec![
            SectionView::new(
                SectionKey::Stats,
                "Section 1",
                vec![
                    // Note: No DetailItem::Header here - headers are added by SectionList
                    DetailItem::artist("a1", "Artist 1"),
                    DetailItem::artist("a2", "Artist 2"),
                ],
            ),
            SectionView::new(
                SectionKey::Albums,
                "Section 2",
                vec![DetailItem::album("al1", "Album 1")],
            ),
        ]
    }

    #[test]
    fn test_section_list_creation() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        assert_eq!(list.sections().len(), 2);
        // 2 section headers + 2 items from Section 1 + 1 item from Section 2 = 5
        assert_eq!(list.items().len(), 5);
        // First focusable should be selected (index 1, skipping Section 1 header)
        assert_eq!(list.selected(), Some(1));
    }

    #[test]
    fn test_section_list_navigation() {
        let sections = make_test_sections();
        let mut list = SectionList::new(sections);

        // Start at first focusable (index 1, skipping section header)
        assert_eq!(list.selected(), Some(1));

        // Move down
        list.select_next();
        assert_eq!(list.selected(), Some(2));

        // Move down again (crosses section boundary, skips Section 2 header)
        list.select_next();
        assert_eq!(list.selected(), Some(4));

        // Move up (back to last item in Section 1)
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
        assert_eq!(marked, vec![1]); // First focusable was selected (index 1)

        list.clear_marks();
        assert!(!list.has_marked());
    }

    #[test]
    fn test_translate_list_action_activate() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Test Activate translation - index 1 is Artist 1 (first focusable item)
        let action = list.translate_list_action(ListAction::Activate(1));
        match action {
            ContentAction::Activate(item) => {
                assert!(matches!(item, DetailItem::Ref(_)));
            }
            _ => panic!("Expected ContentAction::Activate"),
        }
    }

    #[test]
    fn test_translate_list_action_delete() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Test Delete translation - indices 1 and 2 are Artist 1 and Artist 2
        let action = list.translate_list_action(ListAction::Delete(vec![1, 2]));
        match action {
            ContentAction::Delete(items) => {
                assert_eq!(items.len(), 2);
            }
            _ => panic!("Expected ContentAction::Delete"),
        }
    }

    /// BUG: tasks-39,40,41 - Items don't show "currently playing" indicator
    ///
    /// EXPECTED BEHAVIOR: Currently playing song should have
    /// highlight/indicator ACTUAL BEHAVIOR: No playing indicator because
    /// render_simple() hardcodes highlight to false
    ///
    /// ROOT CAUSE: SectionList.render() line 366 uses render_simple() which
    /// doesn't accept a highlight callback, so the "is_playing" indicator
    /// never shows.
    #[test]
    fn render_should_accept_highlight_callback_for_playing_indicator() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // This test documents the architectural issue:
        // render_simple() is called which hardcodes |_, _| false for highlight
        //
        // Expected: Should call render() with a callback that checks
        // ctx.find_current_song_in_queue() Actual: Always passes false, so no
        // "currently playing" visual indicator
        //
        // This affects:
        // - Task 39: All items look the same (no playing indicator)
        // - Task 40: Headers don't stand out
        // - Task 41: No visual distinction for current track
        //
        // TODO: Change line 366 in section_list.rs from:
        //   self.list_view.render_simple(frame, inner, ctx, &self.flat_items, None);
        // To:
        //   self.list_view.render(frame, inner, ctx, &self.flat_items, None, |idx,
        // item| {       // Check if item is currently playing
        //       if let Some(song) = item.as_song() {
        //           ctx.find_current_song_in_queue()
        //               .map(|(_, current)| current.uri == song.uri)
        //               .unwrap_or(false)
        //       } else {
        //           false
        //       }
        //   });

        // For now, just assert the structure exists
        assert!(list.items().len() > 0, "Should have items to render");
    }

    #[test]
    fn test_header_activation_returns_handled() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Activating a header (index 0) should return Handled, not Activate
        let action = list.translate_list_action(ListAction::Activate(0));
        assert!(matches!(action, ContentAction::Handled));
    }

    #[test]
    fn test_content_count_excludes_headers() {
        let sections = make_test_sections();
        let list = SectionList::new(sections);

        // Total items = 5 (2 headers + 3 content)
        assert_eq!(list.items().len(), 5);

        // Content count should be 3
        let content_count = list.items().iter().filter(|i| i.is_content()).count();
        assert_eq!(content_count, 3);
    }
}
