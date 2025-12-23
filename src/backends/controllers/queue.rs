//! Queue management operations
//!
//! Add, remove, reorder, and manage items in the playback queue.

use anyhow::Result;

use crate::backends::api::{Queue as QueueTrait, Item, InsertAt, AfterAdd, Repeat};

/// Manages the playback queue
///
/// # Example
///
/// ```ignore
/// dispatcher.queue().add(&[item], InsertAt::End, AfterAdd::Nothing)?;
/// dispatcher.queue().clear()?;
/// ```
pub struct QueueController<'a> {
    pub(crate) backend: &'a mut dyn QueueTrait,
}

impl QueueController<'_> {
    /// Get all items in the queue
    pub fn list(&mut self) -> Result<Vec<Item>> {
        self.backend.list()
    }

    /// Add items to the queue
    ///
    /// # Arguments
    /// * `items` - Items to add
    /// * `at` - Where to insert (End, Next, Position, Replace)
    /// * `after` - What to do after adding (Nothing, PlayFirst, PlayIndex)
    pub fn add(&mut self, items: &[Item], at: InsertAt, after: AfterAdd) -> Result<()> {
        self.backend.add(items, at, after)
    }

    /// Add a single item at the end of the queue
    pub fn add_one(&mut self, item: &Item) -> Result<()> {
        self.backend.add(&[item.clone()], InsertAt::End, AfterAdd::Nothing)
    }

    /// Add a single item and start playing it
    pub fn add_and_play(&mut self, item: &Item) -> Result<()> {
        self.backend.add(&[item.clone()], InsertAt::End, AfterAdd::PlayFirst)
    }

    /// Remove items from the queue by ID
    pub fn remove(&mut self, queue_ids: &[u32]) -> Result<()> {
        self.backend.remove(queue_ids)
    }

    /// Remove a single item from the queue by ID
    pub fn remove_one(&mut self, queue_id: u32) -> Result<()> {
        self.backend.remove(&[queue_id])
    }

    /// Clear the entire queue
    pub fn clear(&mut self) -> Result<()> {
        self.backend.clear()
    }

    /// Move items to a new position in the queue
    pub fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()> {
        self.backend.move_items(queue_ids, to_position)
    }

    /// Play a specific item in the queue by ID
    pub fn play(&mut self, queue_id: u32) -> Result<()> {
        self.backend.play_id(queue_id)
    }

    /// Set repeat mode
    pub fn set_repeat(&mut self, mode: Repeat) -> Result<()> {
        self.backend.set_repeat(mode)
    }

    /// Set shuffle on/off
    pub fn set_shuffle(&mut self, enabled: bool) -> Result<()> {
        self.backend.set_shuffle(enabled)
    }

    /// Get the number of items in the queue
    pub fn len(&mut self) -> Result<usize> {
        Ok(self.backend.list()?.len())
    }

    /// Check if the queue is empty
    pub fn is_empty(&mut self) -> Result<bool> {
        Ok(self.backend.list()?.is_empty())
    }
}
