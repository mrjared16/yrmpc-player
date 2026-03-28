//! Filter State - Composable filter for list views
//!
//! Provides text-based filtering with match navigation.
//! Designed to be composable with InteractiveListView.

use crate::domain::display::ListItemDisplay;
use crate::shared::string_util::fold_for_match;

/// Filter state for list views
///
/// Tracks filter text, matched indices, and current match position.
/// Enables efficient O(1) navigation between matches.
#[derive(Debug, Clone)]
pub struct FilterState {
    /// Current filter text
    filter_text: String,
    /// Indices of items that match the filter
    matched_indices: Vec<usize>,
    /// Current position in matched_indices (for "[2/15]" display)
    current_match: usize,
}

impl Default for FilterState {
    fn default() -> Self {
        Self { filter_text: String::new(), matched_indices: Vec::new(), current_match: 0 }
    }
}

impl FilterState {
    /// Create a new empty filter state
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with initial filter text
    pub fn with_text(text: &str) -> Self {
        Self { filter_text: text.to_string(), ..Self::default() }
    }

    // ========== TEXT EDITING ==========

    /// Get current filter text
    pub fn text(&self) -> &str {
        &self.filter_text
    }

    /// Check if filter is empty
    pub fn is_empty(&self) -> bool {
        self.filter_text.is_empty()
    }

    /// Set filter text (does NOT recompute matches - call apply() after)
    pub fn set_text(&mut self, text: String) {
        self.filter_text = text;
    }

    /// Push a character (does NOT recompute matches)
    pub fn push_char(&mut self, ch: char) {
        self.filter_text.push(ch);
    }

    /// Pop a character (does NOT recompute matches)
    pub fn pop_char(&mut self) -> Option<char> {
        self.filter_text.pop()
    }

    /// Clear filter text
    pub fn clear(&mut self) {
        self.filter_text.clear();
        self.matched_indices.clear();
        self.current_match = 0;
    }

    // ========== MATCHING ==========

    /// Apply filter to items and compute matched indices
    ///
    /// Uses case-insensitive, diacritic-insensitive substring matching.
    pub fn apply<T: ListItemDisplay>(&mut self, items: &[T]) {
        self.matched_indices.clear();

        if self.filter_text.is_empty() {
            self.current_match = 0;
            return;
        }

        let folded_query = fold_for_match(&self.filter_text);
        if folded_query.is_empty() {
            self.current_match = 0;
            return;
        }

        for (idx, item) in items.iter().enumerate() {
            // Only match focusable items
            if !item.is_focusable() {
                continue;
            }

            if item.matches_folded_query(&folded_query) {
                self.matched_indices.push(idx);
            }
        }

        // Reset current match to 0 if we have matches
        self.current_match = 0;
    }

    /// Check if an index matches the filter
    pub fn is_match(&self, idx: usize) -> bool {
        self.matched_indices.contains(&idx)
    }

    // ========== NAVIGATION ==========

    /// Get current match index (in the items list)
    pub fn current_match_idx(&self) -> Option<usize> {
        self.matched_indices.get(self.current_match).copied()
    }

    /// Navigate to next match, returns the item index
    pub fn next_match(&mut self) -> Option<usize> {
        if self.matched_indices.is_empty() {
            return None;
        }

        self.current_match = (self.current_match + 1) % self.matched_indices.len();
        self.current_match_idx()
    }

    /// Navigate to previous match, returns the item index
    pub fn prev_match(&mut self) -> Option<usize> {
        if self.matched_indices.is_empty() {
            return None;
        }

        if self.current_match == 0 {
            self.current_match = self.matched_indices.len() - 1;
        } else {
            self.current_match -= 1;
        }
        self.current_match_idx()
    }

    /// Jump to specific match that contains or is after item index
    pub fn jump_to_idx(&mut self, idx: usize) {
        if let Some(pos) = self.matched_indices.iter().position(|&i| i >= idx) {
            self.current_match = pos;
        }
    }

    // ========== DISPLAY ==========

    /// Get match count
    pub fn match_count(&self) -> usize {
        self.matched_indices.len()
    }

    /// Get current match position (1-indexed) and total: (2, 15) for "[2/15]"
    pub fn current_of_total(&self) -> (usize, usize) {
        if self.matched_indices.is_empty() {
            (0, 0)
        } else {
            (self.current_match + 1, self.matched_indices.len())
        }
    }

    /// Format display string like "[2/15]" or "[0/0]"
    pub fn display_string(&self) -> String {
        let (current, total) = self.current_of_total();
        format!("[{}/{}]", current, total)
    }

    /// Get all matched indices
    pub fn matched_indices(&self) -> &[usize] {
        &self.matched_indices
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[derive(Debug)]
    struct TestItem {
        text: String,
        secondary: Option<String>,
        focusable: bool,
    }

    impl ListItemDisplay for TestItem {
        fn primary_text(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.text)
        }

        fn secondary_text(&self) -> Option<Cow<'_, str>> {
            self.secondary.as_deref().map(Cow::Borrowed)
        }

        fn is_focusable(&self) -> bool {
            self.focusable
        }
    }

    #[test]
    fn apply_finds_matches_case_insensitive() {
        let items = vec![
            TestItem { text: "Hello World".into(), secondary: None, focusable: true },
            TestItem { text: "Goodbye World".into(), secondary: None, focusable: true },
            TestItem { text: "Hello Again".into(), secondary: None, focusable: true },
        ];

        let mut filter = FilterState::with_text("hello");
        filter.apply(&items);

        assert_eq!(filter.match_count(), 2);
        assert!(filter.is_match(0));
        assert!(!filter.is_match(1));
        assert!(filter.is_match(2));
    }

    #[test]
    fn skips_unfocusable_items() {
        let items = vec![
            TestItem { text: "Header: Hello".into(), secondary: None, focusable: false },
            TestItem { text: "Hello World".into(), secondary: None, focusable: true },
        ];

        let mut filter = FilterState::with_text("hello");
        filter.apply(&items);

        assert_eq!(filter.match_count(), 1);
        assert!(!filter.is_match(0)); // Header skipped
        assert!(filter.is_match(1));
    }

    #[test]
    fn next_match_cycles() {
        let items = vec![
            TestItem { text: "A".into(), secondary: None, focusable: true },
            TestItem { text: "B".into(), secondary: None, focusable: true },
            TestItem { text: "A again".into(), secondary: None, focusable: true },
        ];

        let mut filter = FilterState::with_text("A");
        filter.apply(&items);

        assert_eq!(filter.current_match_idx(), Some(0));
        assert_eq!(filter.next_match(), Some(2)); // Second "A"
        assert_eq!(filter.next_match(), Some(0)); // Wrap to first
    }

    #[test]
    fn display_string_formats_correctly() {
        let items = vec![
            TestItem { text: "Match".into(), secondary: None, focusable: true },
            TestItem { text: "Match".into(), secondary: None, focusable: true },
            TestItem { text: "Match".into(), secondary: None, focusable: true },
        ];

        let mut filter = FilterState::with_text("Match");
        filter.apply(&items);

        assert_eq!(filter.display_string(), "[1/3]");
        filter.next_match();
        assert_eq!(filter.display_string(), "[2/3]");
    }

    #[test]
    fn apply_finds_matches_diacritic_insensitive() {
        let items = vec![
            TestItem { text: "hoàng dũng".into(), secondary: None, focusable: true },
            TestItem { text: "Đặng Thái Sơn".into(), secondary: None, focusable: true },
            TestItem { text: "khác".into(), secondary: None, focusable: true },
        ];

        let mut filter = FilterState::with_text("hoang");
        filter.apply(&items);

        assert_eq!(filter.match_count(), 1);
        assert!(filter.is_match(0));

        filter.set_text("dang".into());
        filter.apply(&items);

        assert_eq!(filter.match_count(), 1);
        assert!(filter.is_match(1));
    }

    #[test]
    fn apply_matches_secondary_text_diacritic_insensitive() {
        let items = vec![TestItem {
            text: "Bài hát".into(),
            secondary: Some("Hoàng Dũng · Live".into()),
            focusable: true,
        }];

        let mut filter = FilterState::with_text("hoang");
        filter.apply(&items);

        assert_eq!(filter.match_count(), 1);
        assert!(filter.is_match(0));
    }
}
