//! Queue management traits and types.
//!
//! Add, remove, reorder, and manage items in the playback queue.
//! Also handles playback behavior modes (single, consume, repeat, shuffle).

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

/// Toggle mode for single/consume behaviors.
///
/// MPD supports `oneshot` variants that apply once then revert to off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToggleMode {
    /// Disabled
    #[default]
    Off,
    /// Enabled permanently
    On,
    /// Apply once then revert to Off (MPD feature)
    Oneshot,
}

impl ToggleMode {
    /// Returns true if mode is active (On or Oneshot)
    pub fn is_active(&self) -> bool {
        matches!(self, ToggleMode::On | ToggleMode::Oneshot)
    }
}

/// Queue management trait
///
/// All operations are bulk-capable for efficiency.
pub trait Queue: Send + Sync {
    /// Add items to queue
    ///
    /// # Arguments
    /// - `items` - Items to add (tracks, or resolved tracks from
    ///   albums/playlists)
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

    // =========================================================================
    // Queue Behavior Modes (optional - default no-op)
    // =========================================================================

    /// Set single mode (stop after current track finishes).
    ///
    /// When enabled, playback stops after the current track completes.
    /// With `Oneshot`, stops once then reverts to normal behavior.
    /// Check `Capability::SingleMode` before using.
    fn set_single(&mut self, mode: ToggleMode) -> Result<()> {
        let _ = mode;
        Ok(())
    }

    /// Set consume mode (remove tracks from queue after playing).
    ///
    /// When enabled, tracks are removed from the queue after playing.
    /// With `Oneshot`, removes once then reverts to normal behavior.
    /// Check `Capability::ConsumeMode` before using.
    fn set_consume(&mut self, mode: ToggleMode) -> Result<()> {
        let _ = mode;
        Ok(())
    }
}
