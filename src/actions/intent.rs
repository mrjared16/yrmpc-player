//! Intent and Selection types for the Action System.
//! Intent and IntentKind - User action requests for the action system.
//!
//! ## Terminology
//!
//! - `IntentKind` (this module): What the user **wants to do** (Play, Queue,
//!   Remove, etc.)
//! - `domain::ActionKind`: What actions a content item **supports** (for UI
//!   action buttons)
//!
//! ## Architecture
//!
//! ```text
//! User presses key
//!     │
//!     ▼
//! Pane builds Intent { action, selection }
//!     │
//!     ▼
//! PaneAction::Execute(Intent)
//!     │
//!     ▼
//! ActionDispatcher routes to Handler
//! ```
//!
//! Selection provides query methods for type validation without extra
//! abstraction.

use std::collections::HashSet;

use crate::domain::{ContentRef, ContentType, DetailItem, Song};

/// The kind of action the user wants to perform.
///
/// Note: UI operations like navigation routing and mark toggling
/// are handled via `PaneAction` directly, not through Intent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IntentKind {
    /// Play the selected content (clears queue, adds songs, plays)
    Play,
    /// Toggle play/pause on current playback (no selection needed)
    TogglePlayback,
    /// Add to queue
    AddToQueue,
    /// Remove from queue
    RemoveFromQueue,
    /// Move up in queue
    MoveUp,
    /// Move down in queue
    MoveDown,
    /// Save to library
    SaveToLibrary,
    /// Start radio from seed song
    StartRadio,
    // Note: Navigate and ToggleMark removed - they are UI operations
    // handled via PaneAction::NavigateTo and SelectableList state
}

/// User intent: what action on what selection.
#[derive(Debug, Clone)]
pub struct Intent {
    /// What action to perform
    pub action: IntentKind,
    /// The selected items
    pub selection: Selection,
}

impl Intent {
    /// Create a new intent.
    pub fn new(action: IntentKind, selection: Selection) -> Self {
        Self { action, selection }
    }

    /// Create a play intent from items.
    pub fn play(items: Vec<DetailItem>) -> Self {
        Self::new(IntentKind::Play, Selection::new(items))
    }

    /// Create a toggle playback intent (no selection needed).
    pub fn toggle_playback() -> Self {
        Self::new(IntentKind::TogglePlayback, Selection::empty())
    }

    /// Create an add-to-queue intent.
    pub fn add_to_queue(items: Vec<DetailItem>) -> Self {
        Self::new(IntentKind::AddToQueue, Selection::new(items))
    }

    /// Create a start-radio intent from a seed song.
    pub fn start_radio(seed: DetailItem) -> Self {
        Self::new(IntentKind::StartRadio, Selection::single(seed))
    }

    // Note: navigate() removed - use PaneAction::NavigateTo instead
}

/// A selection of items to act upon.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// The selected items.
    pub items: Vec<DetailItem>,
}

impl Selection {
    /// Create a new selection from items.
    pub fn new(items: Vec<DetailItem>) -> Self {
        Self { items }
    }

    /// Create a selection with a single item.
    pub fn single(item: DetailItem) -> Self {
        Self { items: vec![item] }
    }

    /// Create an empty selection.
    pub fn empty() -> Self {
        Self { items: vec![] }
    }

    // =========================================================================
    // Query Methods (Selection provides queries, no abstraction needed)
    // =========================================================================

    /// Check if the selection is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get the number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if all items are of the given content types.
    pub fn has_only(&self, types: &[ContentType]) -> bool {
        self.items.iter().all(|item| {
            if let Some(item_type) = item.content_type() {
                types.contains(&item_type)
            } else {
                false // Headers etc. don't have a content type
            }
        })
    }

    /// Check if all items are of the same content type.
    pub fn is_homogeneous(&self) -> bool {
        self.content_types().len() <= 1
    }

    /// Get the set of content types in the selection.
    pub fn content_types(&self) -> HashSet<ContentType> {
        self.items.iter().filter_map(|i| i.content_type()).collect()
    }

    /// Get the primary (first) content type, if any.
    pub fn primary_type(&self) -> Option<ContentType> {
        self.items.first().and_then(|i| i.content_type())
    }

    // =========================================================================
    // Type-specific accessors
    // =========================================================================

    /// Get all songs in the selection.
    pub fn songs(&self) -> Vec<&Song> {
        self.items.iter().filter_map(|i| i.as_song()).collect()
    }

    /// Get all songs, cloned.
    pub fn songs_cloned(&self) -> Vec<Song> {
        self.items.iter().filter_map(|i| i.as_song().cloned()).collect()
    }

    /// Get all content refs in the selection.
    pub fn refs(&self) -> Vec<&ContentRef> {
        self.items.iter().filter_map(|i| i.as_content_ref()).collect()
    }

    /// Check if selection has any songs.
    pub fn has_songs(&self) -> bool {
        self.items.iter().any(|i| i.is_song())
    }

    /// Get the first song, if any.
    pub fn first_song(&self) -> Option<&Song> {
        self.items.iter().find_map(|i| i.as_song())
    }

    /// Find the index of a song by URI in the selection's songs.
    ///
    /// Useful for "play all starting from clicked song" patterns.
    pub fn find_song_index(&self, uri: &str) -> Option<usize> {
        self.songs().iter().position(|s| s.uri == uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_empty() {
        let sel = Selection::empty();
        assert!(sel.is_empty());
        assert_eq!(sel.len(), 0);
    }

    #[test]
    fn test_selection_single() {
        let song = Song::default();
        let sel = Selection::single(DetailItem::Song(song));
        assert!(!sel.is_empty());
        assert_eq!(sel.len(), 1);
        assert!(sel.is_homogeneous());
        assert!(sel.has_songs());
    }

    #[test]
    fn test_selection_has_only() {
        let song = Song::default();
        let sel = Selection::single(DetailItem::Song(song));
        assert!(sel.has_only(&[ContentType::Track]));
        assert!(!sel.has_only(&[ContentType::Album]));
    }
}
