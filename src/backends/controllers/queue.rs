//! Queue management operations
//!
//! Add, remove, reorder, and manage songs in the playback queue.

use anyhow::Result;

use crate::backends::{MusicBackend, QueueOperations};
use crate::domain::{Song, QueuePosition};

/// Manages the playback queue
///
/// # Example
///
/// ```ignore
/// dispatcher.queue().add(&song, None)?;
/// dispatcher.queue().clear()?;
/// dispatcher.queue().shuffle()?;
/// ```
pub struct QueueController<'a> {
    pub(crate) backend: &'a mut dyn MusicBackend,
}

impl QueueController<'_> {
    /// Get all songs in the queue
    pub fn list(&mut self) -> Result<Vec<Song>> {
        self.backend.playlist_info()
    }

    /// Add a song to the queue
    ///
    /// # Arguments
    /// * `song` - The song to add (with full metadata)
    /// * `position` - Where to insert (None = end of queue)
    pub fn add(&mut self, song: &Song, position: Option<QueuePosition>) -> Result<()> {
        self.backend.enqueue(song, position)
    }

    /// Add a song by URI (less metadata, use `add` when possible)
    #[allow(deprecated)]
    pub fn add_uri(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        self.backend.add(uri, position)
    }

    /// Remove a song from the queue by ID
    pub fn remove(&mut self, id: u32) -> Result<()> {
        self.backend.dequeue(id)
    }

    /// Clear the entire queue
    pub fn clear(&mut self) -> Result<()> {
        self.backend.clear_queue()
    }

    /// Move a song to a new position in the queue
    pub fn move_song(&mut self, from_id: u32, to_id: u32) -> Result<()> {
        self.backend.reorder(from_id, to_id)
    }

    /// Play a specific song in the queue by ID
    pub fn play(&mut self, id: u32) -> Result<()> {
        self.backend.play_by_id(id)
    }

    /// Shuffle the queue
    pub fn shuffle(&mut self) -> Result<()> {
        self.backend.shuffle(None)
    }

    /// Get the number of songs in the queue
    pub fn len(&mut self) -> Result<usize> {
        Ok(self.backend.playlist_info()?.len())
    }

    /// Check if the queue is empty
    pub fn is_empty(&mut self) -> Result<bool> {
        Ok(self.backend.playlist_info()?.is_empty())
    }
}
