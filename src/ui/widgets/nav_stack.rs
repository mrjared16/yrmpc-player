//! NavStack - Hierarchical navigation for list views
//!
//! Provides a stack-based navigation interface for entering/leaving nested
//! content. Used by SearchPane for navigating into albums, artists, playlists.
//! Will be extended with DetailStack for Artist/Album/Playlist detail views.
//!
//! ## Design
//!
//! Each level in the stack owns its items and view state independently.
//! This allows:
//! - Independent filtering per level
//! - Preserved selection when leaving and re-entering
//! - Breadcrumb path display

use super::selectable_list::SelectableList;

/// A single level in the navigation stack
#[derive(Debug, Clone)]
pub struct NavLevel<T> {
    /// Items at this level
    pub items: Vec<T>,
    /// View state (selection, marks, filter, etc.)
    pub view: SelectableList,
    /// Path segment for breadcrumb display (e.g., "Albums", "Artist Name")
    pub path_segment: String,
}

impl<T> NavLevel<T> {
    /// Create a new navigation level
    pub fn new(items: Vec<T>, path_segment: String) -> Self {
        let mut view = SelectableList::new();
        // Select first item if available
        if !items.is_empty() {
            view.select(Some(0));
        }
        Self { items, view, path_segment }
    }

    /// Get selected item reference
    pub fn selected_item(&self) -> Option<&T> {
        self.view.selected().and_then(|idx| self.items.get(idx))
    }

    /// Get selected item index and reference
    pub fn selected_with_idx(&self) -> Option<(usize, &T)> {
        self.view.selected().and_then(|idx| self.items.get(idx).map(|item| (idx, item)))
    }
}

/// Stack-based hierarchical navigation
///
/// Enables enter/leave navigation through nested content.
#[derive(Debug, Clone)]
pub struct NavStack<T> {
    /// Stack of browse levels (root is at index 0)
    levels: Vec<NavLevel<T>>,
    /// Separator for path display
    path_separator: &'static str,
}

impl<T> Default for NavStack<T> {
    fn default() -> Self {
        Self { levels: Vec::new(), path_separator: " > " }
    }
}

impl<T> NavStack<T> {
    /// Create empty stack
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with root level
    pub fn with_root(items: Vec<T>, segment: impl Into<String>) -> Self {
        let mut stack = Self::new();
        stack.levels.push(NavLevel::new(items, segment.into()));
        stack
    }

    // ========== NAVIGATION ==========

    /// Enter a new level (push)
    pub fn enter(&mut self, items: Vec<T>, segment: impl Into<String>) {
        self.levels.push(NavLevel::new(items, segment.into()));
    }

    /// Leave current level (pop), returns false if already at root
    pub fn leave(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }

    /// Check if at root level
    pub fn is_at_root(&self) -> bool {
        self.levels.len() <= 1
    }

    /// Get current depth (1 = root)
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Reset to empty stack
    pub fn clear(&mut self) {
        self.levels.clear();
    }

    /// Reset to just root level
    pub fn reset_to_root(&mut self) {
        if self.levels.len() > 1 {
            self.levels.truncate(1);
        }
    }

    // ========== CURRENT LEVEL ACCESS ==========

    /// Get current (top) level reference
    pub fn current(&self) -> Option<&NavLevel<T>> {
        self.levels.last()
    }

    /// Get current level mutable reference
    pub fn current_mut(&mut self) -> Option<&mut NavLevel<T>> {
        self.levels.last_mut()
    }

    /// Get current items slice
    pub fn current_items(&self) -> &[T] {
        self.current().map(|l| l.items.as_slice()).unwrap_or(&[])
    }

    /// Get current items mutable
    pub fn current_items_mut(&mut self) -> &mut Vec<T> {
        self.current_mut().map(|l| &mut l.items).expect("NavStack should have at least one level")
    }

    /// Get current view reference
    pub fn current_view(&self) -> Option<&SelectableList> {
        self.current().map(|l| &l.view)
    }

    /// Get current view mutable reference
    pub fn current_view_mut(&mut self) -> Option<&mut SelectableList> {
        self.current_mut().map(|l| &mut l.view)
    }

    /// Get selected item from current level
    pub fn selected_item(&self) -> Option<&T> {
        self.current().and_then(|l| l.selected_item())
    }

    // ========== PATH DISPLAY ==========

    /// Get full path as string: "Search > Artist > Albums"
    pub fn path(&self) -> String {
        self.levels
            .iter()
            .map(|l| l.path_segment.as_str())
            .collect::<Vec<_>>()
            .join(self.path_separator)
    }

    /// Get path segments as vec
    pub fn path_segments(&self) -> Vec<&str> {
        self.levels.iter().map(|l| l.path_segment.as_str()).collect()
    }

    /// Check if stack is empty
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// Set root level (replaces all)
    pub fn set_root(&mut self, items: Vec<T>, segment: impl Into<String>) {
        self.levels.clear();
        self.levels.push(NavLevel::new(items, segment.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_leave_navigation() {
        let mut stack: NavStack<String> = NavStack::with_root(vec!["a".into(), "b".into()], "Root");

        assert!(stack.is_at_root());
        assert_eq!(stack.depth(), 1);

        stack.enter(vec!["child1".into()], "Level 2");
        assert!(!stack.is_at_root());
        assert_eq!(stack.depth(), 2);

        assert!(stack.leave());
        assert!(stack.is_at_root());

        // Can't leave past root
        assert!(!stack.leave());
    }

    #[test]
    fn path_display() {
        let mut stack: NavStack<i32> = NavStack::with_root(vec![1, 2, 3], "Search");
        stack.enter(vec![10, 20], "Artist");
        stack.enter(vec![100], "Albums");

        assert_eq!(stack.path(), "Search > Artist > Albums");
    }

    #[test]
    fn selected_item_works() {
        let mut stack = NavStack::with_root(vec!["first", "second", "third"], "Root");

        if let Some(view) = stack.current_view_mut() {
            view.select(Some(1));
        }

        assert_eq!(stack.selected_item(), Some(&"second"));
    }
}
