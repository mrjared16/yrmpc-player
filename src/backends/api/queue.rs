//! Queue management traits and types.
//!
//! Add, remove, reorder, and manage items in the playback queue.

use anyhow::Result;
use super::content::Item;

/// Where to insert items in queue
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InsertAt {
    /// Append to end of queue
    #[default]
    End,
    /// After currently playing song
    Next,
    /// At specific position
    Position(u32),
    /// Clear queue first, then add
    Replace,
}

/// What to do after adding items
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AfterAdd {
    /// Don't change playback
    #[default]
    Nothing,
    /// Start playing the first added item
    PlayFirst,
    /// Start playing the Nth added item (0-indexed)
    PlayIndex(usize),
}

/// Queue management trait
///
/// All operations are bulk-capable for efficiency.
pub trait Queue: Send + Sync {
    /// Add items to queue
    ///
    /// # Arguments
    /// - `items` - Items to add (tracks, or resolved tracks from albums/playlists)
    /// - `at` - Where to insert
    /// - `after` - What to do after adding (autoplay)
    ///
    /// # Example
    /// ```ignore
    /// // Add album and play from first track
    /// let tracks = backend.resolve(&album)?;
    /// backend.add(&tracks, InsertAt::End, AfterAdd::PlayFirst)?;
    ///
    /// // Play next (insert after current, don't autoplay)
    /// backend.add(&selected, InsertAt::Next, AfterAdd::Nothing)?;
    /// ```
    fn add(&mut self, items: &[Item], at: InsertAt, after: AfterAdd) -> Result<()>;

    /// Remove items from queue by queue ID
    fn remove(&mut self, queue_ids: &[u32]) -> Result<()>;

    /// Get current queue contents
    fn list(&mut self) -> Result<Vec<Item>>;

    /// Move items to new position
    fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()>;

    /// Clear entire queue
    fn clear(&mut self) -> Result<()>;

    /// Play specific item in queue by ID
    fn play_id(&mut self, queue_id: u32) -> Result<()>;

    /// Set repeat mode
    fn set_repeat(&mut self, mode: super::playback::Repeat) -> Result<()>;

    /// Set shuffle on/off
    fn set_shuffle(&mut self, enabled: bool) -> Result<()>;
}
