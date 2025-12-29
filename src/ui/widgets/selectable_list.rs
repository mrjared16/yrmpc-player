//! Generic interactive list view component for reuse across Queue, Playlist, Search, etc.
//!
//! Provides navigation and rendering for any list of items implementing ListItemDisplay.
//! Actions are handled by the containing Pane (not embedded in this view).
//!
//! ## Design (SOLID - OCP)
//!
//! This component is **closed for modification** but **open for extension**:
//! - Core navigation/selection logic is in ListViewState
//! - Item types implement ListItemDisplay + ItemOps for type-specific behavior
//! - BrowseStack can compose this for hierarchical navigation
//!
//! ## Mode-Aware Key Handling
//!
//! This component supports three input modes (from navigator_types::InputMode):
//! - Normal: Standard navigation (j/k, G/gg, etc.)
//! - Find: Vim-style "/" search with n/N navigation
//! - Edit: Passthrough to pane (e.g., search input)
//!
//! Esc priority: exit mode → clear find → return BackPane
//! Backspace priority: delete char (in Find) → NoEffect (pane handles stack)

use std::borrow::Cow;

use ratatui::{
    layout::Rect,
    widgets::{Block, Borders, ListState},
    Frame,
};

use crate::ctx::Ctx;
use crate::domain::display::ListItemDisplay;
use crate::ui::panes::navigator_types::{InputMode, EscResult, BackspaceResult, ListAction};
use crate::ui::widgets::item_list::{ItemListConfig, ItemListWidget, ListRenderMode};
use crate::ui::widgets::list_view_state::ListViewState;
use crate::ui::widgets::find_state::FindState;
use crate::config::keys::CommonAction;
use crate::shared::key_event::KeyEvent;
use crossterm::event::KeyCode;

/// Wrapper that adds highlight state to any ListItemDisplay item
struct HighlightedItem<'a, T> {
    item: &'a T,
    is_highlighted: bool,
    is_marked: bool,
}

impl<T: ListItemDisplay> ListItemDisplay for HighlightedItem<'_, T> {
    fn primary_text(&self) -> Cow<'_, str> {
        self.item.primary_text()
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        self.item.secondary_text()
    }

    fn thumbnail_url(&self) -> Option<&str> {
        self.item.thumbnail_url()
    }

    fn type_icon(&self) -> &str {
        // Show mark indicator if marked
        if self.is_marked {
            "✓"
        } else {
            self.item.type_icon()
        }
    }

    fn duration_text(&self) -> Option<Cow<'_, str>> {
        self.item.duration_text()
    }

    fn is_playing(&self) -> bool {
        self.is_highlighted
    }

    fn is_focusable(&self) -> bool {
        self.item.is_focusable()
    }
}

/// Configuration for navigation behavior
#[derive(Debug, Clone, Copy)]
pub struct NavConfig {
    pub scrolloff: usize,
    pub wrap: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self { scrolloff: 0, wrap: false }
    }
}

/// Generic interactive list view for any ListItemDisplay items
///
/// Uses ListViewState for state management. Does NOT own items.
/// Optionally supports vim-style find (/) via FindState.
/// 
/// ## Mode Support
/// 
/// The view tracks its own InputMode for Find mode, but Edit mode is
/// managed by the containing pane (for text inputs like search).
#[derive(Debug, Clone)]
pub struct SelectableList {
    /// State (selection, marks, viewport tracking)
    state: ListViewState,
    /// ListState for ratatui widget rendering
    list_state: ListState,
    /// Current input mode (Normal or Find - Edit is pane-managed)
    mode: InputMode,
    /// Optional find state (composable, vim-style / search)
    filter: Option<FindState>,
}

impl Default for SelectableList {
    fn default() -> Self {
        Self {
            state: ListViewState::new(),
            list_state: ListState::default(),
            mode: InputMode::Normal,
            filter: None,
        }
    }
}

impl SelectableList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Access internal state (for advanced use)
    pub fn state(&self) -> &ListViewState {
        &self.state
    }

    /// Access internal state mutably
    pub fn state_mut(&mut self) -> &mut ListViewState {
        &mut self.state
    }

    // ========== NAVIGATION ==========

    /// Move selection down, skipping non-focusable items
    pub fn select_next<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        let len = items.len();
        if len == 0 {
            return;
        }

        // Update content length
        self.state.set_content_and_viewport_len(len, self.state.viewport_len());

        // Try up to len times to find a focusable item
        for _ in 0..len {
            self.state.select_next(cfg.scrolloff, cfg.wrap);
            
            if let Some(idx) = self.state.selected() {
                if items.get(idx).is_some_and(|item| item.is_focusable()) {
                    break;
                }
            }
        }
        
        // Sync ListState
        self.list_state.select(self.state.selected());
    }

    /// Move selection up, skipping non-focusable items
    pub fn select_prev<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        let len = items.len();
        if len == 0 {
            return;
        }

        self.state.set_content_and_viewport_len(len, self.state.viewport_len());

        for _ in 0..len {
            self.state.select_prev(cfg.scrolloff, cfg.wrap);

            if let Some(idx) = self.state.selected() {
                if items.get(idx).is_some_and(|item| item.is_focusable()) {
                    break;
                }
            }
        }

        self.list_state.select(self.state.selected());
    }

    /// Jump to first focusable item
    pub fn select_first<T: ListItemDisplay>(&mut self, items: &[T]) {
        if items.is_empty() {
            return;
        }
        // Find first focusable
        for (idx, item) in items.iter().enumerate() {
            if item.is_focusable() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
                return;
            }
        }
        // Fallback to 0 if no focusable found
        self.state.select(Some(0), 0);
        self.list_state.select(Some(0));
    }

    /// Jump to last focusable item
    pub fn select_last<T: ListItemDisplay>(&mut self, items: &[T]) {
        if items.is_empty() {
            return;
        }
        // Find last focusable
        for (idx, item) in items.iter().enumerate().rev() {
            if item.is_focusable() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
                return;
            }
        }
        // Fallback to last if no focusable found
        let last = items.len() - 1;
        self.state.select(Some(last), 0);
        self.list_state.select(Some(last));
    }

    /// Move down by half viewport
    pub fn next_half_viewport<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        self.state.next_half_viewport(cfg.scrolloff);
        // Skip to focusable if needed
        if let Some(idx) = self.state.selected() {
            if !items.get(idx).is_some_and(|item| item.is_focusable()) {
                self.select_next(items, NavConfig { wrap: false, ..cfg });
            }
        }
        self.list_state.select(self.state.selected());
    }

    /// Move up by half viewport
    pub fn prev_half_viewport<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        self.state.prev_half_viewport(cfg.scrolloff);
        if let Some(idx) = self.state.selected() {
            if !items.get(idx).is_some_and(|item| item.is_focusable()) {
                self.select_prev(items, NavConfig { wrap: false, ..cfg });
            }
        }
        self.list_state.select(self.state.selected());
    }

    /// Move down by full viewport
    pub fn next_viewport<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        self.state.next_viewport(cfg.scrolloff);
        if let Some(idx) = self.state.selected() {
            if !items.get(idx).is_some_and(|item| item.is_focusable()) {
                self.select_next(items, NavConfig { wrap: false, ..cfg });
            }
        }
        self.list_state.select(self.state.selected());
    }

    /// Move up by full viewport
    pub fn prev_viewport<T: ListItemDisplay>(&mut self, items: &[T], cfg: NavConfig) {
        self.state.prev_viewport(cfg.scrolloff);
        if let Some(idx) = self.state.selected() {
            if !items.get(idx).is_some_and(|item| item.is_focusable()) {
                self.select_prev(items, NavConfig { wrap: false, ..cfg });
            }
        }
        self.list_state.select(self.state.selected());
    }

    /// Get currently selected index
    pub fn selected(&self) -> Option<usize> {
        self.state.selected()
    }

    /// Select a specific index
    pub fn select(&mut self, index: Option<usize>) {
        self.state.select(index, 0);
        self.list_state.select(index);
    }

    /// Sync selection to an index if not already set
    pub fn sync_to(&mut self, index: Option<usize>) {
        if self.state.selected().is_none() {
            self.state.select(index, 0);
            self.list_state.select(index);
        }
    }

    /// Update viewport height (call in render or resize)
    pub fn set_viewport_height(&mut self, height: u16) {
        self.state.set_content_and_viewport_len(
            self.state.content_len(),
            height as usize,
        );
    }

    // ========== MODE HANDLING ==========

    /// Get current input mode
    pub fn mode(&self) -> InputMode {
        self.mode
    }

    /// Set input mode (typically called by pane)
    pub fn set_mode(&mut self, mode: InputMode) {
        self.mode = mode;
    }

    /// Check if in Find mode
    pub fn is_find_mode(&self) -> bool {
        self.mode == InputMode::Find
    }

    /// Enter Find mode with optional initial text
    pub fn enter_find_mode<T: ListItemDisplay>(&mut self, items: &[T], initial: &str) {
        self.mode = InputMode::Find;
        self.start_filter(items, initial);
    }

    /// Exit Find mode, optionally keeping the highlights
    pub fn exit_find_mode(&mut self, keep_highlights: bool) {
        self.mode = InputMode::Normal;
        if !keep_highlights {
            self.filter = None;
        }
    }

    /// Handle Esc key with proper priority
    ///
    /// Priority:
    /// 1. Exit Find mode (if in Find mode) → Handled
    /// 2. Clear active filter/highlights (if present) → Handled
    /// 3. Return BackPane (let navigator handle pane history)
    pub fn handle_esc(&mut self) -> EscResult {
        // Priority 1: Exit Find mode
        if self.mode == InputMode::Find {
            self.mode = InputMode::Normal;
            self.filter = None;
            return EscResult::Handled;
        }

        // Priority 2: Clear active filter highlights
        if self.filter.is_some() {
            self.filter = None;
            return EscResult::Handled;
        }

        // Priority 3: Signal to go back pane
        EscResult::BackPane
    }

    /// Handle Backspace key with proper priority
    ///
    /// Priority:
    /// 1. In Find mode: delete char from filter text → Handled
    /// 2. Otherwise: NoEffect (pane handles stack/stage navigation)
    pub fn handle_backspace<T: ListItemDisplay>(&mut self, items: &[T]) -> BackspaceResult {
        // In Find mode, delete character
        if self.mode == InputMode::Find {
            if let Some(ref mut filter) = self.filter {
                if !filter.text().is_empty() {
                    filter.pop_char();
                    filter.apply(items);
                    
                    // Jump to first match if any
                    if let Some(idx) = filter.current_match_idx() {
                        self.state.select(Some(idx), 0);
                        self.list_state.select(Some(idx));
                    }
                    return BackspaceResult::Handled;
                }
            }
            // Empty filter in Find mode - exit find mode
            self.mode = InputMode::Normal;
            self.filter = None;
            return BackspaceResult::Handled;
        }

        // Not in Find mode - pane handles this
        BackspaceResult::NoEffect
    }

    /// Handle character input in Find mode
    /// 
    /// Returns true if character was consumed (in Find mode)
    pub fn handle_find_char<T: ListItemDisplay>(&mut self, items: &[T], ch: char) -> bool {
        if self.mode != InputMode::Find {
            return false;
        }
        
        self.filter_push_char(items, ch);
        true
    }

    // ========== UNIFIED KEY HANDLING ==========

    /// Unified key handler that returns ListAction.
    ///
    /// This is the main entry point for key handling. It processes all keys
    /// and returns an action for the layer above to handle.
    ///
    /// Keys handled:
    /// - j/k/G/gg/Ctrl-d/u: Navigation
    /// - Space: Mark
    /// - Enter: Activate
    /// - /: Enter find mode
    /// - n/N: Find navigation
    /// - d: Delete (queue)
    /// - J/K (shift): Move up/down (queue)
    /// - Esc: Exit mode or bubble Back
    /// - Backspace: Delete char in Find mode or bubble Back
    pub fn handle_key<T: ListItemDisplay>(
        &mut self,
        key: &mut KeyEvent,
        items: &[T],
        ctx: &Ctx,
    ) -> ListAction {
        let cfg = NavConfig {
            scrolloff: ctx.config.scrolloff,
            wrap: ctx.config.wrap_navigation,
        };

        // Handle Find mode
        if self.mode == InputMode::Find {
            match key.code() {
                KeyCode::Esc => {
                    self.exit_find_mode(false);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Enter => {
                    self.exit_find_mode(true);
                    key.stop_propagation();
                    // Return activate on the current selection
                    if let Some(idx) = self.selected() {
                        return ListAction::Activate(idx);
                    }
                    return ListAction::Handled;
                }
                KeyCode::Backspace => {
                    self.handle_backspace(items);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Char('n') => {
                    self.filter_next_match();
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Char('N') => {
                    self.filter_prev_match();
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Char(ch) => {
                    self.filter_push_char(items, ch);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                _ => {}
            }
        }

        // Handle Normal mode with CommonAction
        if let Some(action) = key.as_common_action(ctx) {
            match action {
                CommonAction::Down => {
                    self.select_next(items, cfg);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::Up => {
                    self.select_prev(items, cfg);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::Top => {
                    self.select_first(items);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::Bottom => {
                    self.select_last(items);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::DownHalf => {
                    self.next_half_viewport(items, cfg);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::UpHalf => {
                    self.prev_half_viewport(items, cfg);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                CommonAction::Select => {
                    self.toggle_mark();
                    key.stop_propagation();
                    let marked: Vec<usize> = self.marked_indices().collect();
                    return ListAction::Mark(marked);
                }
                CommonAction::Confirm => {
                    key.stop_propagation();
                    if let Some(idx) = self.selected() {
                        return ListAction::Activate(idx);
                    }
                    return ListAction::Handled;
                }
                CommonAction::Close => {
                    // Esc in Normal mode
                    match self.handle_esc() {
                        EscResult::Handled => {
                            key.stop_propagation();
                            return ListAction::Handled;
                        }
                        EscResult::BackPane => {
                            key.stop_propagation();
                            return ListAction::Back;
                        }
                    }
                }
                _ => {}
            }
        }

        // Handle specific keys not covered by CommonAction
        match key.code() {
            // Find navigation in Normal mode (vim: n/N work after search is confirmed)
            KeyCode::Char('n') if self.filter.is_some() && self.mode == InputMode::Normal => {
                self.filter_next_match();
                key.stop_propagation();
                return ListAction::Handled;
            }
            KeyCode::Char('N') if self.filter.is_some() && self.mode == InputMode::Normal => {
                self.filter_prev_match();
                key.stop_propagation();
                return ListAction::Handled;
            }
            // Find mode entry
            KeyCode::Char('/') => {
                self.enter_find_mode(items, "");
                key.stop_propagation();
                return ListAction::Handled;
            }
            // Delete
            KeyCode::Char('d') => {
                key.stop_propagation();
                let indices = self.get_marked_or_selected();
                if !indices.is_empty() {
                    return ListAction::Delete(indices);
                }
                return ListAction::Handled;
            }
            // Move up (Shift+K)
            KeyCode::Char('K') => {
                key.stop_propagation();
                let indices = self.get_marked_or_selected();
                if !indices.is_empty() {
                    return ListAction::MoveUp(indices);
                }
                return ListAction::Handled;
            }
            // Move down (Shift+J)
            KeyCode::Char('J') => {
                key.stop_propagation();
                let indices = self.get_marked_or_selected();
                if !indices.is_empty() {
                    return ListAction::MoveDown(indices);
                }
                return ListAction::Handled;
            }
            // Backspace
            KeyCode::Backspace => {
                match self.handle_backspace(items) {
                    BackspaceResult::Handled => {
                        key.stop_propagation();
                        return ListAction::Handled;
                    }
                    BackspaceResult::NoEffect => {
                        key.stop_propagation();
                        return ListAction::Back;
                    }
                }
            }
            _ => {}
        }

        ListAction::Passthrough
    }

    /// Get marked indices, or selected index if none marked
    pub fn get_marked_or_selected(&self) -> Vec<usize> {
        if self.has_marked() {
            self.marked_indices().collect()
        } else if let Some(idx) = self.selected() {
            vec![idx]
        } else {
            vec![]
        }
    }

    // ========== MULTI-SELECTION ==========

    /// Toggle mark on currently selected item
    pub fn toggle_mark(&mut self) {
        self.state.toggle_mark();
    }

    /// Mark the currently selected item
    pub fn mark_selected(&mut self) {
        self.state.mark_selected();
    }

    /// Unmark the currently selected item
    pub fn unmark_selected(&mut self) {
        self.state.unmark_selected();
    }

    /// Clear all marks
    pub fn clear_marks(&mut self) {
        self.state.clear_marks();
    }

    /// Check if any items are marked
    pub fn has_marked(&self) -> bool {
        self.state.has_marked()
    }

    /// Get iterator over marked indices
    pub fn marked_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.state.marked_indices()
    }

    /// Get reference to the marked set
    pub fn marked(&self) -> &std::collections::BTreeSet<usize> {
        self.state.marked()
    }

    /// Get mutable reference to the marked set
    pub fn marked_mut(&mut self) -> &mut std::collections::BTreeSet<usize> {
        &mut self.state.marked
    }

    /// Invert all marks (toggle every item)
    pub fn invert_marked(&mut self, len: usize) {
        self.state.set_content_and_viewport_len(len, self.state.viewport_len());
        self.state.invert_marked();
    }

    // ========== FILTERING ==========

    /// Check if filter is active
    pub fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }

    /// Get current filter text
    pub fn filter_text(&self) -> Option<&str> {
        self.filter.as_ref().map(|f| f.text())
    }

    /// Start filtering with initial text
    pub fn start_filter<T: ListItemDisplay>(&mut self, items: &[T], initial: &str) {
        let mut filter = FindState::with_text(initial);
        filter.apply(items);
        
        // Jump to first match if any
        if let Some(idx) = filter.current_match_idx() {
            self.state.select(Some(idx), 0);
            self.list_state.select(Some(idx));
        }
        
        self.filter = Some(filter);
    }

    /// Push a character to filter and reapply
    pub fn filter_push_char<T: ListItemDisplay>(&mut self, items: &[T], ch: char) {
        if let Some(ref mut filter) = self.filter {
            filter.push_char(ch);
            filter.apply(items);
            
            // Jump to first match
            if let Some(idx) = filter.current_match_idx() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
            }
        }
    }

    /// Pop a character from filter and reapply
    pub fn filter_pop_char<T: ListItemDisplay>(&mut self, items: &[T]) {
        if let Some(ref mut filter) = self.filter {
            filter.pop_char();
            filter.apply(items);
            
            // Jump to first match
            if let Some(idx) = filter.current_match_idx() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
            }
        }
    }

    /// Jump to next filter match
    pub fn filter_next_match(&mut self) {
        if let Some(ref mut filter) = self.filter {
            if let Some(idx) = filter.next_match() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
            }
        }
    }

    /// Jump to previous filter match
    pub fn filter_prev_match(&mut self) {
        if let Some(ref mut filter) = self.filter {
            if let Some(idx) = filter.prev_match() {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
            }
        }
    }

    /// Get filter match display string like "[2/15]"
    pub fn filter_display(&self) -> Option<String> {
        self.filter.as_ref().map(|f| f.display_string())
    }

    /// Check if index is a filter match
    pub fn is_filter_match(&self, idx: usize) -> bool {
        self.filter.as_ref().is_some_and(|f| f.is_match(idx))
    }

    /// Clear filter and exit filter mode
    pub fn clear_filter(&mut self) {
        self.filter = None;
    }

    /// Get find state reference (for advanced use)
    pub fn filter_state(&self) -> Option<&FindState> {
        self.filter.as_ref()
    }

    // ========== RENDERING ==========

    /// Render list with optional title and highlight callback
    pub fn render<T, F>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &Ctx,
        items: &[T],
        title: Option<&str>,
        highlight_fn: F,
    )
    where
        T: ListItemDisplay,
        F: Fn(usize, &T) -> bool,
    {
        // Update state with content and viewport info
        let viewport_height = area.height.saturating_sub(2) as usize; // Account for borders
        self.state.set_content_and_viewport_len(items.len(), viewport_height);

        let config = &ctx.config;

        // Create items with highlight and mark context
        let highlighted_items: Vec<HighlightedItem<'_, T>> = items
            .iter()
            .enumerate()
            .map(|(idx, item)| HighlightedItem {
                item,
                is_highlighted: highlight_fn(idx, item),
                is_marked: self.state.marked.contains(&idx),
            })
            .collect();

        // Configure rich list
        let item_config = ItemListConfig {
            mode: ListRenderMode::Rich,
            thumbnail_width: 4,
            row_height: 2,
        };

        let widget = ItemListWidget::new(&highlighted_items, ctx)
            .config(item_config)
            .highlight_style(config.theme.current_item_style);

        // Sync list_state
        self.list_state.select(self.state.selected());

        // Render with optional title
        if let Some(title_text) = title {
            let mark_count = self.state.marked.len();
            let title_str = if mark_count > 0 {
                format!(" {} ({}) [{} marked] ", title_text, items.len(), mark_count)
            } else {
                format!(" {} ({}) ", title_text, items.len())
            };

            let block = Block::default()
                .title(title_str)
                .borders(Borders::ALL)
                .border_style(config.as_border_style());

            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_stateful_widget(widget, inner, &mut self.list_state);
        } else {
            frame.render_stateful_widget(widget, area, &mut self.list_state);
        }
    }

    /// Render without highlight (convenience method)
    pub fn render_simple<T: ListItemDisplay>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &Ctx,
        items: &[T],
        title: Option<&str>,
    ) {
        self.render(frame, area, ctx, items, title, |_, _| false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock item for testing
    #[derive(Debug, Clone)]
    struct TestItem {
        name: String,
        focusable: bool,
    }

    impl ListItemDisplay for TestItem {
        fn primary_text(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.name)
        }
        fn is_focusable(&self) -> bool {
            self.focusable
        }
    }

    #[test]
    fn select_next_skips_unfocusable() {
        let items = vec![
            TestItem { name: "A".into(), focusable: true },
            TestItem { name: "Header".into(), focusable: false },
            TestItem { name: "B".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.select(Some(0));

        view.select_next(&items, NavConfig::default());

        // Should skip index 1 (Header) and land on 2 (B)
        assert_eq!(view.selected(), Some(2));
    }

    #[test]
    fn select_prev_skips_unfocusable() {
        let items = vec![
            TestItem { name: "A".into(), focusable: true },
            TestItem { name: "Header".into(), focusable: false },
            TestItem { name: "B".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.select(Some(2));

        view.select_prev(&items, NavConfig::default());

        // Should skip index 1 (Header) and land on 0 (A)
        assert_eq!(view.selected(), Some(0));
    }

    #[test]
    fn toggle_mark_works() {
        let mut view = SelectableList::new();
        view.select(Some(5));

        view.toggle_mark();
        assert!(view.state.marked.contains(&5));

        view.toggle_mark();
        assert!(!view.state.marked.contains(&5));
    }

    #[test]
    fn marked_indices_returns_all_marked() {
        let mut view = SelectableList::new();
        view.state.marked.insert(1);
        view.state.marked.insert(3);
        view.state.marked.insert(5);

        let indices: Vec<_> = view.marked_indices().collect();
        assert_eq!(indices, vec![1, 3, 5]);
    }
}
