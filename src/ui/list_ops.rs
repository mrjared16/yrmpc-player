//! Shared operations for list-based UIs
//!
//! Provides reusable functions for executing actions on selected/marked items.
//! This module enables bulk operations across Queue, Playlist, and Search
//! panes.
//!
//! ## Design (SOLID - OCP)
//!
//! - Functions are generic over `T: ItemOps`
//! - Panes provide the items slice and view state
//! - Action execution is delegated to the item's ItemOps implementation
//!
//! ## Borrow Safety
//!
//! Because Rust's borrow checker won't let us borrow `&ctx.queue_state()` and
//! `&mut ctx` simultaneously, these functions take cloned items. Callers should
//! clone only the necessary items (e.g., `ctx.queue_state().songs().to_vec()`
//! for the full list, or just the selected item).

use anyhow::Result;

use crate::{
    ctx::Ctx,
    domain::{QueueItemAction, QueueItemOps, Song},
    shared::macros::status_error,
    ui::widgets::selectable_list::SelectableList,
};

/// Execute an action on the currently selected item
///
/// Note: Takes owned items to avoid borrow conflicts with ctx.
/// Callers should clone the items first: `ctx.queue_state().songs().to_vec()`
pub fn execute_on_selected(
    view: &SelectableList,
    items: Vec<Song>,
    action: QueueItemAction,
    ctx: &mut Ctx,
) -> Result<bool> {
    if let Some(idx) = view.selected() {
        if let Some(item) = items.get(idx) {
            item.execute_queue_action(action, ctx)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Execute an action on all marked items
///
/// Processes items in reverse order (important for delete operations).
/// Clears marks after execution.
/// Note: Takes owned items to avoid borrow conflicts with ctx.
pub fn execute_on_marked(
    view: &mut SelectableList,
    items: Vec<Song>,
    action: QueueItemAction,
    ctx: &mut Ctx,
) -> Result<usize> {
    if !view.has_marked() {
        return Ok(0);
    }

    let indices: Vec<_> = view.marked_indices().collect();
    let count = indices.len();

    // Process in reverse order for safe deletion
    for idx in indices.into_iter().rev() {
        if let Some(item) = items.get(idx) {
            // Ignore individual errors, continue with rest
            let _ = item.execute_queue_action(action, ctx);
        }
    }

    view.clear_marks();
    Ok(count)
}

/// Execute an action on marked items if any, otherwise on selected item
///
/// This is the common pattern: if marks exist, batch operation; else single
/// item. Note: Takes owned items to avoid borrow conflicts with ctx.
pub fn execute_on_marked_or_selected(
    view: &mut SelectableList,
    items: Vec<Song>,
    action: QueueItemAction,
    ctx: &mut Ctx,
) -> Result<usize> {
    if view.has_marked() {
        execute_on_marked(view, items, action, ctx)
    } else if execute_on_selected(view, items, action, ctx)? {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Direction for move operations
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDirection {
    Up,
    Down,
}

/// Execute a move action on the selected item and update selection to follow
///
/// Returns true if the move was executed.
/// This is the shared implementation for QueuePaneV2 and QueueModal.
pub fn execute_move(
    view: &mut SelectableList,
    items: &[Song],
    direction: MoveDirection,
    ctx: &mut Ctx,
) -> bool {
    use crate::domain::QueueItemAction;

    if let Some(idx) = view.selected() {
        if let Some(song) = items.get(idx).cloned() {
            let action = match direction {
                MoveDirection::Up => QueueItemAction::MoveUp,
                MoveDirection::Down => QueueItemAction::MoveDown,
            };

            if song.execute_queue_action(action, ctx).is_ok() {
                // Update selection to follow the moved item
                let new_idx = match direction {
                    MoveDirection::Up if idx > 0 => idx - 1,
                    MoveDirection::Down if idx < items.len().saturating_sub(1) => idx + 1,
                    _ => idx,
                };
                view.select(Some(new_idx));
                return true;
            }
        }
    }
    false
}

// =============================================================================
// Selection Pattern - Unified single/bulk operation interface
// =============================================================================

/// Represents selected items for bulk operations.
/// Uses indices (not references) to avoid lifetime/borrow conflicts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Single item selected (by index)
    Single(usize),
    /// Multiple items marked (by indices)
    Multiple(Vec<usize>),
}

/// Get the current selection from a list view.
/// Returns marked items if any are marked, otherwise the selected item.
pub fn get_selection(view: &SelectableList) -> Option<Selection> {
    if view.has_marked() {
        let indices: Vec<_> = view.marked_indices().collect();
        if indices.is_empty() { None } else { Some(Selection::Multiple(indices)) }
    } else {
        view.selected().map(Selection::Single)
    }
}

// =============================================================================
// QueueListBehavior - Shared behavior trait for queue views
// =============================================================================

/// Behavior trait for views that display and interact with ctx.queue_state().
/// Provides shared action logic for play, delete, and move operations.
///
/// ## Usage
///
/// Implement this trait for any view that shows the queue:
/// ```ignore
/// impl QueueListBehavior for QueuePaneV2 {
///     fn list_view(&self) -> &InteractiveListView { &self.list_view }
///     fn list_view_mut(&mut self) -> &mut InteractiveListView { &mut self.list_view }
/// }
/// ```
///
/// Then call the trait methods from handle_action:
/// ```ignore
/// CommonAction::Confirm => QueueListBehavior::play_selected(self, ctx),
/// CommonAction::Delete => QueueListBehavior::delete_selected(self, ctx),
/// ```
pub trait QueueListBehavior {
    /// Access to the underlying list view (immutable)
    fn list_view(&self) -> &SelectableList;

    /// Access to the underlying list view (mutable)
    fn list_view_mut(&mut self) -> &mut SelectableList;

    /// Play or toggle the selected song.
    /// Default: Uses QueueItemAction::PlayOrToggle for Spotify-style behavior.
    /// NOTE: Uses fresh ctx.queue_state() to avoid stale data issues.
    fn play_selected(&mut self, ctx: &mut Ctx) {
        if let Some(idx) = self.list_view().selected() {
            // Clone only the single song we need, then access ctx is free
            if let Some(song) = ctx.queue_state().get(idx) {
                if let Err(err) = song.execute_queue_action(QueueItemAction::PlayOrToggle, ctx) {
                    status_error!("{}", err);
                }
            }
        }
    }

    /// Delete selected/marked songs from the queue.
    /// Processes in reverse order for safe index-based deletion.
    /// NOTE: Uses fresh ctx.queue_state() to avoid stale data issues.
    fn delete_selected(&mut self, ctx: &mut Ctx) {
        let old_idx = self.list_view().selected();

        let count = match get_selection(self.list_view()) {
            Some(Selection::Single(i)) => {
                // Clone only the single song we need
                if let Some(song) = ctx.queue_state().get(i) {
                    match song.execute_queue_action(QueueItemAction::Delete, ctx) {
                        Ok(()) => 1,
                        Err(err) => {
                            status_error!("{}", err);
                            0
                        }
                    }
                } else {
                    0
                }
            }
            Some(Selection::Multiple(indices)) => {
                // Process in reverse order for safe deletion
                // Clone songs we need BEFORE any mutations
                let songs_to_delete: Vec<_> =
                    indices.iter().rev().filter_map(|&idx| ctx.queue_state().get(idx)).collect();

                let mut count = 0;
                for song in songs_to_delete {
                    match song.execute_queue_action(QueueItemAction::Delete, ctx) {
                        Ok(()) => count += 1,
                        Err(err) => status_error!("{}", err),
                    }
                }

                if count > 0 {
                    self.list_view_mut().clear_marks();
                }

                count
            }
            None => 0,
        };

        if count > 0 {
            self.on_after_delete(ctx, old_idx, count);
        }
    }

    /// Move the selected song up or down in the queue.
    /// NOTE: Uses fresh ctx.queue_state() to avoid stale data issues.
    fn move_selected(&mut self, direction: MoveDirection, ctx: &mut Ctx) {
        if let Some(idx) = self.list_view().selected() {
            if let Some(song) = ctx.queue_state().get(idx) {
                let action = match direction {
                    MoveDirection::Up => QueueItemAction::MoveUp,
                    MoveDirection::Down => QueueItemAction::MoveDown,
                };

                match song.execute_queue_action(action, ctx) {
                    Ok(()) => {
                        // Update selection to follow the moved item
                        let queue_len = ctx.queue_state().len();
                        let new_idx = match direction {
                            MoveDirection::Up if idx > 0 => idx - 1,
                            MoveDirection::Down if idx < queue_len.saturating_sub(1) => idx + 1,
                            _ => idx,
                        };
                        self.list_view_mut().select(Some(new_idx));
                    }
                    Err(err) => status_error!("{}", err),
                }
            }
        }
    }

    /// Hook called after deletion completes.
    /// Override to add custom behavior (e.g., close modal if queue empty).
    /// Default: Adjusts selection to previous item if needed.
    fn on_after_delete(&mut self, ctx: &mut Ctx, old_idx: Option<usize>, _count: usize) {
        // Adjust selection after delete
        if let Some(idx) = old_idx {
            let queue_len = ctx.queue_state().len();
            if queue_len > 0 {
                let new_idx = idx.min(queue_len.saturating_sub(1));
                self.list_view_mut().select(Some(new_idx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_single() {
        // Would need InteractiveListView mock
    }

    #[test]
    fn test_selection_multiple() {
        // Would need InteractiveListView mock
    }
}
