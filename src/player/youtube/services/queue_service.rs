//! Queue service - manages playback queue

use anyhow::Result;
use parking_lot::Mutex;
use std::collections::VecDeque;
use crate::domain::Song;

/// Queue item with unique ID
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: u32,
    pub song: Song,
}

/// Queue service manages playback queue and current position
pub struct QueueService {
    queue: Mutex<VecDeque<QueueItem>>,
    current_idx: Mutex<Option<usize>>,
    next_id: Mutex<u32>,
}

impl QueueService {
    /// Create new queue service
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            current_idx: Mutex::new(None),
            next_id: Mutex::new(1),
        }
    }

    /// Add song to queue, returns assigned ID
    pub fn add(&self, mut song: Song, position: Option<u32>) -> u32 {
        let mut queue = self.queue.lock();
        let mut next_id = self.next_id.lock();
        
        let id = *next_id;
        *next_id += 1;

        // Set the song's ID so it can be looked up later
        song.id = Some(id);
        
        let item = QueueItem { id, song };

        match position {
            Some(pos) => {
                let insert_pos = (pos as usize).min(queue.len());
                queue.insert(insert_pos, item);
            }
            None => {
                queue.push_back(item);
            }
        }

        id
    }

    /// Remove song by ID
    pub fn remove(&self, id: u32) -> Result<()> {
        let mut queue = self.queue.lock();
        let pos = queue.iter().position(|item| item.id == id)
            .ok_or_else(|| anyhow::anyhow!("Song not found"))?;
        queue.remove(pos);
        Ok(())
    }

    /// Clear entire queue
    pub fn clear(&self) {
        self.queue.lock().clear();
        *self.current_idx.lock() = None;
    }

    /// Get song by ID
    pub fn get_by_id(&self, id: u32) -> Result<Song> {
        let queue = self.queue.lock();
        queue.iter()
            .find(|item| item.id == id)
            .map(|item| item.song.clone())
            .ok_or_else(|| anyhow::anyhow!("Song not found"))
    }

    /// Get song by index
    pub fn get_by_index(&self, idx: usize) -> Result<Song> {
        let queue = self.queue.lock();
        queue.get(idx)
            .map(|item| item.song.clone())
            .ok_or_else(|| anyhow::anyhow!("Position out of bounds"))
    }

    /// Get all songs in queue
    pub fn get_all(&self) -> Vec<Song> {
        self.queue.lock()
            .iter()
            .map(|item| item.song.clone())
            .collect()
    }

    /// Get current playing index
    pub fn current_index(&self) -> Option<usize> {
        *self.current_idx.lock()
    }

    /// Set current playing index
    pub fn set_current(&self, idx: Option<usize>) {
        *self.current_idx.lock() = idx;
    }

    /// Get next song index (if exists)
    pub fn next_index(&self) -> Option<usize> {
        let current = *self.current_idx.lock();
        let len = self.queue.lock().len();
        
        current.and_then(|idx| {
            if idx + 1 < len {
                Some(idx + 1)
            } else {
                None
            }
        })
    }

    /// Get previous song index (if exists)
    pub fn previous_index(&self) -> Option<usize> {
        self.current_idx.lock().and_then(|idx| {
            if idx > 0 {
                Some(idx - 1)
            } else {
                None
            }
        })
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }

    /// Move song from one ID to another position
    pub fn move_song(&self, from_id: u32, to_id: u32) -> Result<()> {
        let mut queue = self.queue.lock();
        let from_pos = queue.iter().position(|item| item.id == from_id);
        let to_pos = queue.iter().position(|item| item.id == to_id);

        match (from_pos, to_pos) {
            (Some(from), Some(to)) => {
                if let Some(item) = queue.remove(from) {
                    let insert_pos = to.min(queue.len());
                    queue.insert(insert_pos, item);
                }
                Ok(())
            }
            _ => Err(anyhow::anyhow!("Song not found")),
        }
    }
}
