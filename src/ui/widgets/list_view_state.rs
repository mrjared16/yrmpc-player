//! List View State - Extracted state management for InteractiveListView
//!
//! This module contains the core state tracking for list-based UIs,
//! including selection, marks, viewport, and scrolloff handling.
//!
//! Designed to be compatible with ranger-style navigation (from DirState).

use std::collections::BTreeSet;

use ratatui::widgets::ScrollbarState;

/// State for an interactive list view
///
/// Tracks selection, multi-selection marks, viewport dimensions,
/// and scroll offset for proper scrollbar rendering.
#[derive(Debug, Clone)]
pub struct ListViewState {
    /// Currently selected index
    selected: Option<usize>,
    /// Set of marked (multi-selected) indices
    pub marked: BTreeSet<usize>,
    /// Scroll offset (first visible item)
    offset: usize,
    /// Total number of items in content
    content_len: usize,
    /// Number of visible items in viewport
    viewport_len: usize,
    /// Scrollbar widget state
    scrollbar: ScrollbarState,
}

impl Default for ListViewState {
    fn default() -> Self {
        Self {
            selected: None,
            marked: BTreeSet::new(),
            offset: 0,
            content_len: 0,
            viewport_len: 0,
            scrollbar: ScrollbarState::default(),
        }
    }
}

impl ListViewState {
    pub fn new() -> Self {
        Self::default()
    }

    // ========== GETTERS ==========

    /// Get currently selected index
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Get scroll offset
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Get content length
    pub fn content_len(&self) -> usize {
        self.content_len
    }

    /// Get viewport length
    pub fn viewport_len(&self) -> usize {
        self.viewport_len
    }

    /// Get scrollbar state reference for rendering
    pub fn scrollbar_state(&mut self) -> &mut ScrollbarState {
        &mut self.scrollbar
    }

    // ========== SETTERS ==========

    /// Set content and viewport length (call on resize or data change)
    pub fn set_content_and_viewport_len(&mut self, content_len: usize, viewport_len: usize) {
        self.content_len = content_len;
        self.viewport_len = viewport_len;
        self.scrollbar = self
            .scrollbar
            .content_length(content_len)
            .viewport_content_length(viewport_len);

        // Clamp offset if content shrunk
        if self.offset > 0 && self.offset + viewport_len > content_len {
            self.offset = content_len.saturating_sub(viewport_len);
        }
    }

    /// Select a specific index with scrolloff
    pub fn select(&mut self, idx: Option<usize>, scrolloff: usize) {
        self.selected = idx;
        if let Some(i) = idx {
            self.ensure_visible(i, scrolloff);
        }
    }

    /// Ensure an index is visible with scrolloff margin
    fn ensure_visible(&mut self, idx: usize, scrolloff: usize) {
        if self.viewport_len == 0 || self.content_len == 0 {
            return;
        }

        // Calculate effective scrolloff (can't be more than half viewport)
        let effective_scrolloff = scrolloff.min(self.viewport_len / 2);

        // Check if item is above viewport (with scrolloff)
        if idx < self.offset + effective_scrolloff {
            self.offset = idx.saturating_sub(effective_scrolloff);
        }
        // Check if item is below viewport (with scrolloff)
        else if idx >= self.offset + self.viewport_len - effective_scrolloff {
            self.offset = (idx + 1 + effective_scrolloff).saturating_sub(self.viewport_len);
        }

        // Clamp offset
        self.offset = self.offset.min(self.content_len.saturating_sub(self.viewport_len));
        self.scrollbar = self.scrollbar.position(self.offset);
    }

    // ========== NAVIGATION ==========

    /// Move selection down, respecting scrolloff
    pub fn select_next(&mut self, scrolloff: usize, wrap: bool) {
        let len = self.content_len;
        if len == 0 {
            return;
        }

        let current = self.selected.unwrap_or(0);
        let next = if current + 1 >= len {
            if wrap { 0 } else { len - 1 }
        } else {
            current + 1
        };

        self.select(Some(next), scrolloff);
    }

    /// Move selection up, respecting scrolloff
    pub fn select_prev(&mut self, scrolloff: usize, wrap: bool) {
        let len = self.content_len;
        if len == 0 {
            return;
        }

        let current = self.selected.unwrap_or(0);
        let prev = if current == 0 {
            if wrap { len - 1 } else { 0 }
        } else {
            current - 1
        };

        self.select(Some(prev), scrolloff);
    }

    /// Jump to first item
    pub fn select_first(&mut self, scrolloff: usize) {
        if self.content_len > 0 {
            self.select(Some(0), scrolloff);
        }
    }

    /// Jump to last item
    pub fn select_last(&mut self, scrolloff: usize) {
        if self.content_len > 0 {
            self.select(Some(self.content_len - 1), scrolloff);
        }
    }

    /// Move down by half viewport
    pub fn next_half_viewport(&mut self, scrolloff: usize) {
        let half = (self.viewport_len / 2).max(1);
        let current = self.selected.unwrap_or(0);
        let target = (current + half).min(self.content_len.saturating_sub(1));
        self.select(Some(target), scrolloff);
    }

    /// Move up by half viewport
    pub fn prev_half_viewport(&mut self, scrolloff: usize) {
        let half = (self.viewport_len / 2).max(1);
        let current = self.selected.unwrap_or(0);
        let target = current.saturating_sub(half);
        self.select(Some(target), scrolloff);
    }

    /// Move down by full viewport
    pub fn next_viewport(&mut self, scrolloff: usize) {
        let current = self.selected.unwrap_or(0);
        let target = (current + self.viewport_len).min(self.content_len.saturating_sub(1));
        self.select(Some(target), scrolloff);
    }

    /// Move up by full viewport
    pub fn prev_viewport(&mut self, scrolloff: usize) {
        let current = self.selected.unwrap_or(0);
        let target = current.saturating_sub(self.viewport_len);
        self.select(Some(target), scrolloff);
    }

    /// Map a rendered row to content index
    pub fn get_at_rendered_row(&self, row: usize) -> Option<usize> {
        let idx = self.offset + row;
        if idx < self.content_len {
            Some(idx)
        } else {
            None
        }
    }

    // ========== MARKS ==========

    /// Toggle mark on selected item
    pub fn toggle_mark(&mut self) {
        if let Some(idx) = self.selected {
            if self.marked.contains(&idx) {
                self.marked.remove(&idx);
            } else {
                self.marked.insert(idx);
            }
        }
    }

    /// Mark the selected item
    pub fn mark_selected(&mut self) {
        if let Some(idx) = self.selected {
            self.marked.insert(idx);
        }
    }

    /// Unmark the selected item
    pub fn unmark_selected(&mut self) {
        if let Some(idx) = self.selected {
            self.marked.remove(&idx);
        }
    }

    /// Clear all marks
    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// Check if any items are marked
    pub fn has_marked(&self) -> bool {
        !self.marked.is_empty()
    }

    /// Invert all marks
    pub fn invert_marked(&mut self) {
        let mut new_marked = BTreeSet::new();
        for i in 0..self.content_len {
            if !self.marked.contains(&i) {
                new_marked.insert(i);
            }
        }
        self.marked = new_marked;
    }

    /// Mark all items
    pub fn mark_all(&mut self) {
        for i in 0..self.content_len {
            self.marked.insert(i);
        }
    }

    /// Get marked indices iterator
    pub fn marked_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.marked.iter().copied()
    }

    /// Get marked set reference
    pub fn marked(&self) -> &BTreeSet<usize> {
        &self.marked
    }

    /// Adjust marks after removing an item
    pub fn remove_item(&mut self, idx: usize) {
        // Remove the mark if it exists
        self.marked.remove(&idx);

        // Shift marks above the removed index
        let shifted: BTreeSet<usize> = self
            .marked
            .iter()
            .map(|&i| if i > idx { i - 1 } else { i })
            .collect();
        self.marked = shifted;

        // Update content length
        if self.content_len > 0 {
            self.content_len -= 1;
        }

        // Adjust selection if needed
        if let Some(sel) = self.selected {
            if sel >= self.content_len && self.content_len > 0 {
                self.selected = Some(self.content_len - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_with_scrolloff_scrolls_down() {
        let mut state = ListViewState::new();
        state.set_content_and_viewport_len(100, 10);

        // Select item near end of viewport
        state.select(Some(8), 2);

        // Should scroll to keep 2-item margin
        assert!(state.offset > 0);
    }

    #[test]
    fn select_with_scrolloff_scrolls_up() {
        let mut state = ListViewState::new();
        state.set_content_and_viewport_len(100, 10);
        state.offset = 50;

        // Select item near start of viewport
        state.select(Some(51), 2);

        // Should scroll to keep 2-item margin
        assert!(state.offset < 50);
    }

    #[test]
    fn half_viewport_moves_correctly() {
        let mut state = ListViewState::new();
        state.set_content_and_viewport_len(100, 10);
        state.select(Some(0), 0);

        state.next_half_viewport(0);

        assert_eq!(state.selected(), Some(5));
    }

    #[test]
    fn get_at_rendered_row_works() {
        let mut state = ListViewState::new();
        state.set_content_and_viewport_len(100, 10);
        state.offset = 20;

        assert_eq!(state.get_at_rendered_row(0), Some(20));
        assert_eq!(state.get_at_rendered_row(5), Some(25));
    }

    #[test]
    fn remove_item_shifts_marks() {
        let mut state = ListViewState::new();
        state.set_content_and_viewport_len(10, 5);
        state.marked.insert(3);
        state.marked.insert(5);
        state.marked.insert(7);

        state.remove_item(4);

        // 5 and 7 should shift to 4 and 6
        assert!(state.marked.contains(&3));
        assert!(state.marked.contains(&4)); // was 5
        assert!(state.marked.contains(&6)); // was 7
        assert!(!state.marked.contains(&5));
    }
}
