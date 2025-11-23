use std::collections::VecDeque;
use std::time::Instant;
use anyhow::{Result, bail};

use crate::domain::{Song, QueuePosition};

/// Represents an item in the application queue
#[derive(Debug, Clone)]
pub struct QueueItem {
    /// Unique ID for this queue item (auto-incremented)
    pub id: u32,
    /// The song in this queue position
    pub song: Song,
    /// When this item was added to the queue
    pub added_at: Instant,
}

impl QueueItem {
    pub fn new(id: u32, song: Song) -> Self {
        Self {
            id,
            song,
            added_at: Instant::now(),
        }
    }
}

/// Application state managing the playback queue
/// 
/// This struct maintains an in-memory queue independent of the backend,
/// allowing for consistent queue management across different backends
/// (MPD, YouTube Music, etc.)
#[derive(Debug)]
pub struct AppState {
    /// The queue of songs
    queue: VecDeque<QueueItem>,
    
    /// Current playing position (index into queue)
    current_index: Option<usize>,
    
    /// Queue version - incremented on any change
    version: u32,
    
    /// Last modification time
    last_modified: Instant,
    
    /// Next available ID for queue items
    next_id: u32,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Create a new empty AppState
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            current_index: None,
            version: 0,
            last_modified: Instant::now(),
            next_id: 1,
        }
    }

    // ===== Queue Operations =====

    /// Add a song to the queue at the specified position
    /// Returns the ID assigned to this queue item
    pub fn add(&mut self, song: Song, position: Option<QueuePosition>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        
        let item = QueueItem::new(id, song);
        
        match position {
            None | Some(QueuePosition::End) => {
                // Add to end of queue
                self.queue.push_back(item);
            }
            Some(QueuePosition::Absolute(pos)) => {
                let insert_pos = (pos as usize).min(self.queue.len());
                self.queue.insert(insert_pos, item);
                
                // Adjust current_index if needed
                if let Some(current) = self.current_index {
                    if current >= insert_pos {
                        self.current_index = Some(current + 1);
                    }
                }
            }
            Some(QueuePosition::Next) => {
                // Add after current song
                if let Some(current) = self.current_index {
                    let insert_pos = (current + 1).min(self.queue.len());
                    self.queue.insert(insert_pos, item);
                } else {
                    self.queue.push_back(item);
                }
            }
            Some(QueuePosition::Relative(offset)) => {
                // Add relative to current position
                if let Some(current) = self.current_index {
                    let new_pos = if offset >= 0 {
                        current.saturating_add(offset as usize)
                    } else {
                        current.saturating_sub(offset.abs() as usize)
                    };
                    let insert_pos = new_pos.min(self.queue.len());
                    self.queue.insert(insert_pos, item);
                } else {
                    self.queue.push_back(item);
                }
            }
        }
        
        self.bump_version();
        id
    }

    /// Delete a queue item by ID
    pub fn delete_id(&mut self, id: u32) -> Result<()> {
        if let Some(pos) = self.queue.iter().position(|item| item.id == id) {
            self.queue.remove(pos);
            
            // Adjust current_index if needed
            if let Some(current) = self.current_index {
                if current == pos {
                    self.current_index = None;
                } else if current > pos {
                    self.current_index = Some(current - 1);
                }
            }
            
            self.bump_version();
            Ok(())
        } else {
            bail!("Queue item with ID {} not found", id)
        }
    }

    /// Clear the entire queue
    pub fn clear(&mut self) {
        self.queue.clear();
        self.current_index = None;
        self.bump_version();
    }

    /// Move a queue item from one ID to another position
    pub fn move_id(&mut self, from_id: u32, to_id: u32) -> Result<()> {
        let from_pos = self.queue.iter().position(|item| item.id == from_id)
            .ok_or_else(|| anyhow::anyhow!("Source item with ID {} not found", from_id))?;
        
        let to_pos = self.queue.iter().position(|item| item.id == to_id)
            .ok_or_else(|| anyhow::anyhow!("Target item with ID {} not found", to_id))?;
        
        if from_pos == to_pos {
            return Ok(());
        }
        
        let item = self.queue.remove(from_pos).unwrap();
        
        // Adjust to_pos if removing affected it
        let adjusted_to_pos = if from_pos < to_pos {
            to_pos - 1
        } else {
            to_pos
        };
        
        self.queue.insert(adjusted_to_pos, item);
        
        // Adjust current_index
        if let Some(current) = self.current_index {
            self.current_index = Some(match current {
                c if c == from_pos => adjusted_to_pos,
                c if from_pos < c && c <= to_pos => c - 1,
                c if to_pos <= c && c < from_pos => c + 1,
                c => c,
            });
        }
        
        self.bump_version();
        Ok(())
    }

    // ===== Query Operations =====

    /// Get the entire queue as a slice
    pub fn get_queue(&self) -> &VecDeque<QueueItem> {
        &self.queue
    }

    /// Get a queue item by ID
    pub fn find_by_id(&self, id: u32) -> Option<&QueueItem> {
        self.queue.iter().find(|item| item.id == id)
    }

    /// Get the current playing item
    pub fn get_current(&self) -> Option<&QueueItem> {
        self.current_index.and_then(|idx| self.queue.get(idx))
    }

    /// Get the next item in the queue
    pub fn get_next(&self) -> Option<&QueueItem> {
        self.current_index.and_then(|idx| self.queue.get(idx + 1))
    }

    /// Get the current index
    pub fn get_current_index(&self) -> Option<usize> {
        self.current_index
    }

    /// Set the current playing index
    pub fn set_current_index(&mut self, index: usize) -> Result<()> {
        if index < self.queue.len() {
            self.current_index = Some(index);
            Ok(())
        } else {
            bail!("Index {} out of bounds (queue length: {})", index, self.queue.len())
        }
    }

    /// Set the current playing item by ID
    pub fn set_current_by_id(&mut self, id: u32) -> Result<()> {
        if let Some(pos) = self.queue.iter().position(|item| item.id == id) {
            self.current_index = Some(pos);
            Ok(())
        } else {
            bail!("Queue item with ID {} not found", id)
        }
    }

    /// Get the queue version (increments on any change)
    pub fn get_version(&self) -> u32 {
        self.version
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    // ===== Bulk Operations =====

    /// Replace the entire queue with a new one
    /// Useful for syncing from backend
    pub fn replace_queue(&mut self, songs: Vec<Song>) {
        self.queue.clear();
        self.current_index = None;
        
        for song in songs {
            let id = self.next_id;
            self.next_id += 1;
            self.queue.push_back(QueueItem::new(id, song));
        }
        
        self.bump_version();
    }

    // ===== Internal Helpers =====

    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.last_modified = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_song(name: &str) -> Song {
        Song {
            file: format!("{}.mp3", name),
            metadata: HashMap::from([
                ("title".to_string(), vec![name.to_string()]),
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn test_add_to_end() {
        let mut state = AppState::new();
        let song1 = create_test_song("song1");
        let song2 = create_test_song("song2");
        
        let id1 = state.add(song1, None);
        let id2 = state.add(song2, None);
        
        assert_eq!(state.len(), 2);
        assert_eq!(state.get_queue()[0].id, id1);
        assert_eq!(state.get_queue()[1].id, id2);
    }

    #[test]
    fn test_add_at_position() {
        let mut state = AppState::new();
        let song1 = create_test_song("song1");
        let song2 = create_test_song("song2");
        let song3 = create_test_song("song3");
        
        state.add(song1, None);
        state.add(song2, None);
        let id3 = state.add(song3, Some(QueuePosition::Absolute(1)));
        
        assert_eq!(state.len(), 3);
        assert_eq!(state.get_queue()[1].id, id3);
    }

    #[test]
    fn test_delete_by_id() {
        let mut state = AppState::new();
        let song1 = create_test_song("song1");
        let song2 = create_test_song("song2");
        
        let id1 = state.add(song1, None);
        state.add(song2, None);
        
        assert!(state.delete_id(id1).is_ok());
        assert_eq!(state.len(), 1);
        assert!(state.delete_id(999).is_err());
    }

    #[test]
    fn test_clear() {
        let mut state = AppState::new();
        state.add(create_test_song("song1"), None);
        state.add(create_test_song("song2"), None);
        
        state.clear();
        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn test_move_items() {
        let mut state = AppState::new();
        let id1 = state.add(create_test_song("song1"), None);
        let id2 = state.add(create_test_song("song2"), None);
        let id3 = state.add(create_test_song("song3"), None);
        
        // Move song1 to position of song3
        assert!(state.move_id(id1, id3).is_ok());
        
        // Order should now be: song2, song3, song1
        assert_eq!(state.get_queue()[0].id, id2);
        assert_eq!(state.get_queue()[1].id, id3);
        assert_eq!(state.get_queue()[2].id, id1);
    }

    #[test]
    fn test_current_index() {
        let mut state = AppState::new();
        let id1 = state.add(create_test_song("song1"), None);
        state.add(create_test_song("song2"), None);
        
        assert!(state.get_current().is_none());
        
        assert!(state.set_current_by_id(id1).is_ok());
        assert_eq!(state.get_current_index(), Some(0));
        assert!(state.get_current().is_some());
    }

    #[test]
    fn test_version_tracking() {
        let mut state = AppState::new();
        let initial_version = state.get_version();
        
        state.add(create_test_song("song1"), None);
        assert!(state.get_version() > initial_version);
        
        let version_after_add = state.get_version();
        state.clear();
        assert!(state.get_version() > version_after_add);
    }

    #[test]
    fn test_replace_queue() {
        let mut state = AppState::new();
        state.add(create_test_song("old1"), None);
        
        let new_songs = vec![
            create_test_song("new1"),
            create_test_song("new2"),
        ];
        
        state.replace_queue(new_songs);
        assert_eq!(state.len(), 2);
        assert!(state.get_queue()[0].song.file.contains("new1"));
    }
}
