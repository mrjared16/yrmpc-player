//! Generic interactive list view component for reuse across Queue, Playlist,
//! Search, etc.
//!
//! Provides navigation and rendering for any list of items implementing
//! ListItemDisplay. Actions are handled by the containing Pane (not embedded in
//! this view).
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

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, ListState},
};

use crate::{
    config::keys::CommonAction,
    ctx::Ctx,
    domain::display::ListItemDisplay,
    shared::key_event::KeyEvent,
    ui::{
        panes::navigator_types::{BackspaceResult, EscResult, InputMode, ListAction},
        widgets::{
            find_state::FindState,
            item_list::{ItemListConfig, ItemListWidget, ListRenderMode},
            list_view_state::ListViewState,
        },
    },
};

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
        if self.is_marked { "✓" } else { self.item.type_icon() }
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

    fn is_header(&self) -> bool {
        self.item.is_header()
    }

    fn icon_style(&self) -> ratatui::style::Style {
        self.item.icon_style()
    }

    fn search_key(&self) -> crate::domain::display::SearchKey {
        self.item.search_key()
    }

    fn matches_folded_query(&self, folded_query: &str) -> bool {
        self.item.matches_folded_query(folded_query)
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
        self.state.set_content_and_viewport_len(self.state.content_len(), height as usize);
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
        let keep_active_search =
            keep_highlights && self.filter.as_ref().is_some_and(|filter| !filter.text().is_empty());
        if !keep_active_search {
            self.filter = None;
        }
    }

    /// Confirm the active find and jump to the next match.
    pub fn confirm_find_and_jump_to_next(&mut self) {
        if let Some(ref mut filter) = self.filter {
            if let Some(idx) = filter.next_match().or_else(|| filter.current_match_idx()) {
                self.state.select(Some(idx), 0);
                self.list_state.select(Some(idx));
            }
        }
        self.exit_find_mode(true);
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
            self.exit_find_mode(true);
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
        let cfg = NavConfig { scrolloff: ctx.config.scrolloff, wrap: ctx.config.wrap_navigation };

        // Handle Find mode
        if self.mode == InputMode::Find {
            match key.code() {
                KeyCode::Esc => {
                    self.exit_find_mode(true);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Enter => {
                    self.exit_find_mode(true);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Tab => {
                    self.confirm_find_and_jump_to_next();
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                KeyCode::Backspace => {
                    self.handle_backspace(items);
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                // NOTE: 'n'/'N' for next/prev match are NOT handled here in Find mode.
                // All characters (including n/N) should be typed into the filter pattern.
                // n/N navigation only works in Normal mode AFTER filter is confirmed (see below).
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
            // Add to queue ('a' key)
            KeyCode::Char('a') => {
                key.stop_propagation();
                let indices = self.get_marked_or_selected();
                if !indices.is_empty() {
                    return ListAction::Enqueue(indices);
                }
                return ListAction::Handled;
            }
            // Backspace
            KeyCode::Backspace => match self.handle_backspace(items) {
                BackspaceResult::Handled => {
                    key.stop_propagation();
                    return ListAction::Handled;
                }
                BackspaceResult::NoEffect => {
                    key.stop_propagation();
                    return ListAction::Back;
                }
            },
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

    fn sync_filter_to_items<T: ListItemDisplay>(&mut self, items: &[T]) {
        let previous_selection = self.selected();
        let Some(ref mut filter) = self.filter else {
            return;
        };
        filter.apply(items);

        if let Some(selected_idx) = previous_selection.filter(|&idx| idx < items.len()) {
            if filter.is_match(selected_idx) {
                filter.jump_to_idx(selected_idx);
                return;
            }
        }

        if let Some(idx) = filter.current_match_idx() {
            self.state.select(Some(idx), 0);
            self.list_state.select(Some(idx));
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
    ) where
        T: ListItemDisplay,
        F: Fn(usize, &T) -> bool,
    {
        self.sync_filter_to_items(items);

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
        let item_config =
            ItemListConfig { mode: ListRenderMode::Rich, thumbnail_width: 4, row_height: 2 };

        let filter_match_indices = self.filter.as_ref().map(FindState::matched_indices);

        let widget = ItemListWidget::new(&highlighted_items, ctx)
            .config(item_config)
            .highlight_style(config.theme.current_item_style)
            .filter_match_indices(filter_match_indices);

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

    // ============================================================================
    // TDD Bug Regression Tests
    // These tests prove bugs exist (should FAIL before fix)
    // ============================================================================

    /// BUG: task-43 - 'a' key (add to queue) not working
    ///
    /// EXPECTED BEHAVIOR: Pressing 'a' should return
    /// ListAction::Enqueue(indices) ACTUAL BEHAVIOR: Returns
    /// ListAction::Passthrough (key not handled)
    ///
    /// ROOT CAUSE: SelectableList.handle_key() doesn't handle 'a' key
    #[test]
    fn handle_key_a_returns_enqueue_action() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{
            shared::key_event::KeyEvent, tests::fixtures::ctx,
            ui::panes::navigator_types::ListAction,
        };

        let items = vec![
            TestItem { name: "Song 1".into(), focusable: true },
            TestItem { name: "Song 2".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.select(Some(0));

        // Create a KeyEvent for 'a' key
        let crossterm_key = CKeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let mut key = KeyEvent::from(crossterm_key);
        let ctx = ctx();
        let action = view.handle_key(&mut key, &items, &ctx);

        // This test verifies 'a' key returns Enqueue action
        // After fix: should return ListAction::Enqueue with selected indices
        assert!(
            matches!(action, ListAction::Enqueue(_)),
            "Expected 'a' key to return Enqueue action, got {:?}",
            action
        );

        // Verify the indices are correct
        if let ListAction::Enqueue(indices) = action {
            assert_eq!(indices, vec![0], "Should enqueue selected item at index 0");
        }
    }

    /// BUG: task-42 - Find mode no highlight on matches
    ///
    /// EXPECTED BEHAVIOR: is_filter_match() returns true for items matching
    /// filter ACTUAL BEHAVIOR: Filter state not properly exposed/used in
    /// rendering
    #[test]
    fn filter_match_is_detected_for_matching_item() {
        let items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Banana".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let mut view = SelectableList::new();

        // Enter find mode with "ap"
        view.enter_find_mode(&items, "ap");

        // Check if filter is active
        assert!(view.is_filtering(), "Should be in filter mode");
        assert_eq!(view.filter_text(), Some("ap"), "Filter text should be 'ap'");

        // Test filter matching - this exposes the rendering bug
        // The filter exists but is_filter_match isn't used properly in render
        assert!(view.is_filter_match(0), "Apple should match 'ap' (case-insensitive)");
        assert!(!view.is_filter_match(1), "Banana should NOT match 'ap'");
        assert!(view.is_filter_match(2), "Apricot should match 'ap' (case-insensitive)");
    }

    // ============================================================================
    // Task-48: Find mode cannot type 'n' character
    // ============================================================================

    /// BUG: task-48 - Find mode: Cannot type 'n' character
    ///
    /// EXPECTED BEHAVIOR: Pressing 'n' in Find mode adds 'n' to filter pattern
    /// ACTUAL BEHAVIOR: 'n' triggers filter_next_match() instead of typing
    ///
    /// ROOT CAUSE: KeyCode::Char('n') is matched before KeyCode::Char(ch) in
    /// Find mode
    ///
    /// This test should FAIL before fix, PASS after fix.
    #[test]
    fn find_mode_typing_n_adds_to_filter() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{
            shared::key_event::KeyEvent, tests::fixtures::ctx,
            ui::panes::navigator_types::ListAction,
        };

        let items = vec![
            TestItem { name: "Song One".into(), focusable: true },
            TestItem { name: "Another Song".into(), focusable: true },
            TestItem { name: "None".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.select(Some(0));

        // Enter Find mode with initial text "so"
        view.enter_find_mode(&items, "so");
        assert_eq!(view.mode, InputMode::Find, "Should be in Find mode");
        assert_eq!(view.filter_text(), Some("so"), "Initial filter should be 'so'");

        // Now press 'n' - this should ADD 'n' to the filter, making it "son"
        let crossterm_key = CKeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        let mut key = KeyEvent::from(crossterm_key);
        let ctx = ctx();
        let _action = view.handle_key(&mut key, &items, &ctx);

        // THE CRITICAL ASSERTION:
        // After pressing 'n' in Find mode, filter should be "son"
        // BUG: Currently 'n' triggers filter_next_match() so filter stays "so"
        assert_eq!(
            view.filter_text(),
            Some("son"),
            "BUG: Pressing 'n' in Find mode should add 'n' to filter, not jump to next match. Filter is {:?}",
            view.filter_text()
        );
    }

    /// BUG: task-48 - 'N' should also be typeable in Find mode
    #[test]
    fn find_mode_typing_capital_n_adds_to_filter() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{shared::key_event::KeyEvent, tests::fixtures::ctx};

        let items = vec![TestItem { name: "Name".into(), focusable: true }];
        let mut view = SelectableList::new();
        view.select(Some(0));

        // Enter Find mode
        view.enter_find_mode(&items, "");

        // Press 'N' (capital)
        let crossterm_key = CKeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT);
        let mut key = KeyEvent::from(crossterm_key);
        let ctx = ctx();
        let _action = view.handle_key(&mut key, &items, &ctx);

        // Should add 'N' to filter
        assert_eq!(
            view.filter_text(),
            Some("N"),
            "BUG: Pressing 'N' in Find mode should add 'N' to filter"
        );
    }

    #[test]
    fn esc_in_find_mode_confirms_search() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{shared::key_event::KeyEvent, tests::fixtures::ctx};

        let items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.enter_find_mode(&items, "ap");

        let mut key = KeyEvent::from(CKeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let ctx = ctx();
        let action = view.handle_key(&mut key, &items, &ctx);

        assert!(matches!(action, ListAction::Handled));
        assert_eq!(view.mode(), InputMode::Normal);
        assert!(view.is_filtering());
        assert_eq!(view.filter_text(), Some("ap"));
    }

    #[test]
    fn enter_in_find_mode_confirms_without_activation() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{shared::key_event::KeyEvent, tests::fixtures::ctx};

        let items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.select(Some(0));
        view.enter_find_mode(&items, "ap");

        let mut key = KeyEvent::from(CKeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let ctx = ctx();
        let action = view.handle_key(&mut key, &items, &ctx);

        assert!(matches!(action, ListAction::Handled));
        assert_eq!(view.mode(), InputMode::Normal);
        assert!(view.is_filtering());
    }

    #[test]
    fn n_navigates_after_esc_confirms_search() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{shared::key_event::KeyEvent, tests::fixtures::ctx};

        let items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Banana".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.enter_find_mode(&items, "ap");
        assert_eq!(view.selected(), Some(0));

        let ctx = ctx();
        let mut esc = KeyEvent::from(CKeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let _ = view.handle_key(&mut esc, &items, &ctx);

        let mut n_key = KeyEvent::from(CKeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        let action = view.handle_key(&mut n_key, &items, &ctx);

        assert!(matches!(action, ListAction::Handled));
        assert_eq!(view.selected(), Some(2));
    }

    #[test]
    fn tab_in_find_mode_confirms_and_jumps_to_next_match() {
        use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

        use crate::{shared::key_event::KeyEvent, tests::fixtures::ctx};

        let items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Banana".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let mut view = SelectableList::new();
        view.enter_find_mode(&items, "ap");
        assert_eq!(view.selected(), Some(0));

        let mut tab = KeyEvent::from(CKeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let ctx = ctx();
        let action = view.handle_key(&mut tab, &items, &ctx);

        assert!(matches!(action, ListAction::Handled));
        assert_eq!(view.mode(), InputMode::Normal);
        assert!(view.is_filtering());
        assert_eq!(view.filter_text(), Some("ap"));
        assert_eq!(view.selected(), Some(2));
    }

    #[test]
    fn active_filter_recomputes_when_items_change() {
        let initial_items = vec![
            TestItem { name: "Apple".into(), focusable: true },
            TestItem { name: "Banana".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];
        let updated_items = vec![
            TestItem { name: "Berry".into(), focusable: true },
            TestItem { name: "Apricot".into(), focusable: true },
        ];

        let mut view = SelectableList::new();
        view.enter_find_mode(&initial_items, "ap");
        view.exit_find_mode(true);
        assert_eq!(view.selected(), Some(0));
        assert!(view.is_filter_match(0));
        assert!(view.is_filter_match(2));

        view.sync_filter_to_items(&updated_items);

        assert_eq!(view.selected(), Some(1));
        assert!(!view.is_filter_match(0));
        assert!(view.is_filter_match(1));
    }
}
