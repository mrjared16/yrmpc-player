//! Queue service - manages playback queue metadata
//!
//! Architecture: Dual-layer system
//! - QueueService: Stores metadata (title, artist, thumbnail) for UI display
//! - MPV: Owns the actual playlist (URLs, playback order, current position)
//!
//! The queue_id is stable (survives reorders), while MPV index is ephemeral.

use std::collections::VecDeque;

use anyhow::Result;
use parking_lot::Mutex;

use crate::domain::Song;

/// Repeat mode for the queue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    /// No repeat - stop at end of queue
    #[default]
    Off,
    /// Repeat current track
    One,
    /// Repeat entire queue (loop back to start)
    All,
}

/// Queue item with unique ID
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: u32,
    pub song: Song,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackWindowState {
    pub current_idx: Option<usize>,
    pub playback_base_index: usize,
    pub prefetch_indices: Vec<usize>,
}

/// Queue service manages playback queue and current position
pub struct QueueService {
    queue: Mutex<VecDeque<QueueItem>>,
    playback_window: Mutex<PlaybackWindowState>,
    next_id: Mutex<u32>,
    repeat_mode: Mutex<RepeatMode>,
    shuffle_enabled: Mutex<bool>,
    /// History of played indices for shuffle mode's "previous" functionality
    shuffle_history: Mutex<Vec<usize>>,
    /// Pre-computed shuffle order (None = sequential mode)
    /// Contains queue indices in shuffled playback order
    shuffle_order: Mutex<Option<Vec<usize>>>,
}

impl QueueService {
    /// Create new queue service
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            playback_window: Mutex::new(PlaybackWindowState::default()),
            next_id: Mutex::new(1),
            repeat_mode: Mutex::new(RepeatMode::Off),
            shuffle_enabled: Mutex::new(false),
            shuffle_history: Mutex::new(Vec::new()),
            shuffle_order: Mutex::new(None),
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

        let insert_pos = match position {
            Some(pos) => {
                let insert_pos = (pos as usize).min(queue.len());
                queue.insert(insert_pos, item);
                insert_pos
            }
            None => {
                let insert_pos = queue.len();
                queue.push_back(item);
                insert_pos
            }
        };

        let mut playback_window = self.playback_window.lock();
        if let Some(current) = playback_window.current_idx {
            if insert_pos <= current {
                playback_window.current_idx = Some(current + 1);
            }

            if insert_pos <= playback_window.playback_base_index {
                playback_window.playback_base_index += 1;
            }
        }

        id
    }

    /// Remove song by ID, returns the removed item and its index (for MPV sync)
    pub fn remove(&self, id: u32) -> Result<(QueueItem, usize)> {
        let mut queue = self.queue.lock();
        let pos = queue
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| anyhow::anyhow!("Song not found"))?;
        let item = queue.remove(pos).ok_or_else(|| anyhow::anyhow!("Failed to remove song"))?;

        // Update current_idx if needed
        let mut playback_window = self.playback_window.lock();
        if let Some(curr) = playback_window.current_idx {
            if pos < curr {
                playback_window.current_idx = Some(curr - 1);
            } else if pos == curr {
                playback_window.current_idx = None;
            }
        }

        if pos < playback_window.playback_base_index {
            playback_window.playback_base_index =
                playback_window.playback_base_index.saturating_sub(1);
        }

        Ok((item, pos))
    }

    /// Clear entire queue
    pub fn clear(&self) {
        self.queue.lock().clear();
        *self.playback_window.lock() = PlaybackWindowState::default();
    }

    /// Get song by ID
    pub fn get_by_id(&self, id: u32) -> Result<Song> {
        let queue = self.queue.lock();
        queue
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.song.clone())
            .ok_or_else(|| anyhow::anyhow!("Song not found"))
    }

    /// Get song by index
    pub fn get_by_index(&self, idx: usize) -> Result<Song> {
        let queue = self.queue.lock();
        queue
            .get(idx)
            .map(|item| item.song.clone())
            .ok_or_else(|| anyhow::anyhow!("Position out of bounds"))
    }

    /// Find index of a queue item by its ID (useful for MPV playlist sync)
    pub fn find_index_by_id(&self, id: u32) -> Option<usize> {
        self.queue.lock().iter().position(|item| item.id == id)
    }

    /// Get all songs in queue
    pub fn get_all(&self) -> Vec<Song> {
        self.queue.lock().iter().map(|item| item.song.clone()).collect()
    }

    /// Get current playing index
    pub fn current_index(&self) -> Option<usize> {
        self.playback_window.lock().current_idx
    }

    /// Set current playing index
    /// In shuffle mode, also updates the shuffle history
    pub fn set_current(&self, idx: Option<usize>) {
        self.playback_window.lock().current_idx = idx;

        // Update shuffle history if enabled and we have a valid index
        if let Some(new_idx) = idx {
            if *self.shuffle_enabled.lock() {
                let mut history = self.shuffle_history.lock();
                // Avoid duplicates at end of history
                if history.last() != Some(&new_idx) {
                    history.push(new_idx);
                    // Keep history bounded to avoid memory growth
                    let max_history = 100;
                    if history.len() > max_history {
                        let drain_count = history.len() - max_history;
                        history.drain(0..drain_count);
                    }
                }
            }
        }
    }

    /// Get playback base index
    ///
    /// This is the queue index that corresponds to MPV's playlist[0].
    /// Use this (not current_index) when calculating positions from
    /// mpv_playlist_pos.
    pub fn playback_base_index(&self) -> usize {
        self.playback_window.lock().playback_base_index
    }

    /// Set playback base index
    ///
    /// Call this when rebuilding MPV's playlist (e.g., in play_position).
    /// The position should be the queue index of the first track loaded into
    /// MPV.
    pub fn set_playback_base_index(&self, pos: usize) {
        self.playback_window.lock().playback_base_index = pos;
    }

    pub fn set_playback_window_state(
        &self,
        current_idx: Option<usize>,
        playback_base_index: usize,
        prefetch_indices: Vec<usize>,
    ) {
        *self.playback_window.lock() =
            PlaybackWindowState { current_idx, playback_base_index, prefetch_indices };
    }

    pub fn playback_window_state(&self) -> PlaybackWindowState {
        self.playback_window.lock().clone()
    }

    /// Get next song index (if exists)
    /// In shuffle mode, returns a random unplayed or least-recently-played
    /// track
    pub fn next_index(&self) -> Option<usize> {
        let current = self.playback_window.lock().current_idx;
        let len = self.queue.lock().len();

        if len == 0 {
            return None;
        }

        if *self.shuffle_enabled.lock() {
            // Shuffle mode: pick random track, avoiding recent plays
            self.pick_shuffle_next(current, len)
        } else {
            // Sequential mode
            current.and_then(|idx| if idx + 1 < len { Some(idx + 1) } else { None })
        }
    }

    /// Pick next track in shuffle mode
    /// Avoids recently played tracks and picks randomly from remaining
    fn pick_shuffle_next(&self, current: Option<usize>, len: usize) -> Option<usize> {
        use rand::Rng;

        if len <= 1 {
            return if len == 1 { Some(0) } else { None };
        }

        let history = self.shuffle_history.lock();

        // Build list of candidates: all tracks not in recent history
        // Recent = last min(len/2, history.len()) tracks
        let history_window = (len / 2).min(history.len()).max(1);
        let recent: std::collections::HashSet<usize> =
            history.iter().rev().take(history_window).copied().collect();

        // Add current to recent if playing
        let mut avoid = recent;
        if let Some(curr) = current {
            avoid.insert(curr);
        }

        // Candidates are all indices not in avoid set
        let candidates: Vec<usize> = (0..len).filter(|i| !avoid.contains(i)).collect();

        if candidates.is_empty() {
            // All tracks played recently, pick any except current
            let fallback: Vec<usize> = (0..len).filter(|i| Some(*i) != current).collect();
            if fallback.is_empty() {
                Some(0) // Only one track
            } else {
                let idx = rand::thread_rng().gen_range(0..fallback.len());
                Some(fallback[idx])
            }
        } else {
            let idx = rand::thread_rng().gen_range(0..candidates.len());
            Some(candidates[idx])
        }
    }

    /// Get previous song index (if exists)
    /// In shuffle mode, goes back through shuffle history
    pub fn previous_index(&self) -> Option<usize> {
        if *self.shuffle_enabled.lock() {
            // Shuffle mode: go back in history
            let mut history = self.shuffle_history.lock();
            if history.len() > 1 {
                history.pop(); // Remove current
                history.last().copied()
            } else {
                None
            }
        } else {
            // Sequential mode
            self.playback_window
                .lock()
                .current_idx
                .and_then(|idx| if idx > 0 { Some(idx - 1) } else { None })
        }
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }

    /// Get current repeat mode
    pub fn repeat_mode(&self) -> RepeatMode {
        *self.repeat_mode.lock()
    }

    /// Set repeat mode
    pub fn set_repeat_mode(&self, mode: RepeatMode) {
        *self.repeat_mode.lock() = mode;
    }

    /// Cycle to next repeat mode: Off -> All -> One -> Off
    pub fn cycle_repeat_mode(&self) -> RepeatMode {
        let mut mode = self.repeat_mode.lock();
        *mode = match *mode {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        *mode
    }

    /// Get shuffle enabled state
    pub fn shuffle_enabled(&self) -> bool {
        *self.shuffle_enabled.lock()
    }

    /// Set shuffle enabled state
    /// When enabling: regenerates shuffle_order, clears prefetch_indices,
    /// starts fresh history When disabling: clears shuffle_order,
    /// prefetch_indices, and history
    pub fn set_shuffle_enabled(&self, enabled: bool) {
        *self.shuffle_enabled.lock() = enabled;

        if enabled {
            // Start fresh history with current track if playing
            let current = self.playback_window.lock().current_idx;
            let mut history = self.shuffle_history.lock();
            history.clear();
            if let Some(idx) = current {
                history.push(idx);
            }

            // Regenerate shuffle order for immediate effect
            let len = self.len();
            if len > 0 {
                *self.shuffle_order.lock() =
                    Some(self.generate_shuffle_order_internal(len, current));
            }

            // Clear stale prefetch indices so next prefetch uses new shuffle order
            self.playback_window.lock().prefetch_indices.clear();
        } else {
            // Clear history when disabling shuffle
            self.shuffle_history.lock().clear();
            // Clear shuffle order
            *self.shuffle_order.lock() = None;
            // Clear stale prefetch indices so next prefetch uses sequential order
            self.playback_window.lock().prefetch_indices.clear();
        }
    }

    /// Toggle shuffle and return new state
    pub fn toggle_shuffle(&self) -> bool {
        let mut enabled = self.shuffle_enabled.lock();
        *enabled = !*enabled;
        *enabled
    }

    /// Move song from one ID to another position, returns (from_idx, to_idx)
    /// for MPV sync
    ///
    /// Semantics: "Put the FROM song at the position currently occupied by TO
    /// song"
    /// - If FROM is before TO: TO and everything between shift up, FROM takes
    ///   TO's old spot
    /// - If FROM is after TO: TO and everything between shift down, FROM takes
    ///   TO's old spot
    pub fn move_song(&self, from_id: u32, to_id: u32) -> Result<(usize, usize)> {
        let mut queue = self.queue.lock();
        let from_pos = queue.iter().position(|item| item.id == from_id);
        let to_pos = queue.iter().position(|item| item.id == to_id);

        match (from_pos, to_pos) {
            (Some(from), Some(to)) => {
                if from == to {
                    // No-op: moving to same position
                    return Ok((from, to));
                }

                // Check if we're moving the current track
                let mut playback_window = self.playback_window.lock();
                let was_current = playback_window.current_idx == Some(from);

                if let Some(item) = queue.remove(from) {
                    // After removal, positions shift:
                    // - If from < to: target position shifts down by 1
                    // - If from > to: target position unchanged
                    let insert_pos =
                        if from < to { (to - 1).min(queue.len()) } else { to.min(queue.len()) };
                    queue.insert(insert_pos, item);

                    // Update current_idx if the currently playing track moved
                    if was_current {
                        playback_window.current_idx = Some(insert_pos);
                    } else if let Some(curr) = playback_window.current_idx {
                        // Adjust current_idx if tracks shifted around it
                        if from < curr && to >= curr {
                            playback_window.current_idx = Some(curr - 1);
                        } else if from > curr && to <= curr {
                            playback_window.current_idx = Some(curr + 1);
                        }
                    }

                    let bi = playback_window.playback_base_index;
                    if from == bi {
                        playback_window.playback_base_index = insert_pos;
                    } else if from < bi && insert_pos >= bi {
                        playback_window.playback_base_index = bi.saturating_sub(1);
                    } else if from > bi && insert_pos <= bi {
                        playback_window.playback_base_index = bi + 1;
                    }

                    Ok((from, insert_pos))
                } else {
                    Err(anyhow::anyhow!("Failed to remove song"))
                }
            }
            _ => Err(anyhow::anyhow!("Song not found")),
        }
    }

    // =========================================================================
    // PREFETCH WINDOW MANAGEMENT (for gapless playback with shuffle/repeat)
    // =========================================================================

    /// Build prefetch window starting from a queue index.
    ///
    /// Returns queue indices to load into MPV, respecting shuffle order.
    /// Also stores the indices in `prefetch_indices` for later lookup.
    ///
    /// If shuffle is enabled but shuffle_order doesn't exist, generates it.
    pub fn build_prefetch_window(&self, start_idx: usize, count: usize) -> Vec<usize> {
        self.ensure_shuffle_order(start_idx);
        let indices = self.compute_prefetch_window(start_idx, count);
        self.set_prefetch_indices(indices.clone());
        indices
    }

    pub fn ensure_shuffle_order(&self, start_idx: usize) {
        let len = self.len();
        if len == 0 || !*self.shuffle_enabled.lock() {
            return;
        }

        let mut order = self.shuffle_order.lock();
        if order.is_none() || order.as_ref().map(|o| o.len()) != Some(len) {
            *order = Some(self.generate_shuffle_order_internal(len, Some(start_idx)));
        }
    }

    pub fn compute_prefetch_window(&self, start_idx: usize, count: usize) -> Vec<usize> {
        let len = self.len();
        if len == 0 {
            return Vec::new();
        }

        let mut indices = Vec::with_capacity(count);
        let repeat_mode = *self.repeat_mode.lock();

        indices.push(start_idx);

        for i in 1..count {
            if let Some(next_idx) = self.get_next_in_playback_order(start_idx, i, repeat_mode) {
                indices.push(next_idx);
            } else {
                break;
            }
        }

        indices
    }

    pub fn compute_background_extract_scope(&self, start_idx: usize) -> Vec<usize> {
        let len = self.len();
        if len == 0 {
            return Vec::new();
        }

        self.ensure_shuffle_order(start_idx);

        (0..len)
            .filter_map(|offset| {
                self.get_next_in_playback_order(start_idx, offset, RepeatMode::All)
            })
            .collect()
    }

    pub fn set_prefetch_indices(&self, indices: Vec<usize>) {
        self.playback_window.lock().prefetch_indices = indices;
    }

    /// Get the queue index at a given position in the current prefetch window.
    ///
    /// This is the KEY method that fixes the shuffle bug:
    /// Instead of `base_index + mpv_pos`, we look up from stored indices.
    pub fn get_prefetched_at(&self, mpv_pos: usize) -> Option<usize> {
        self.playback_window.lock().prefetch_indices.get(mpv_pos).copied()
    }

    /// Extend the prefetch window by one track and return the new queue index.
    ///
    /// Called after auto-advance to maintain the rolling window.
    pub fn extend_prefetch_window(&self) -> Option<usize> {
        let mut playback_window = self.playback_window.lock();
        let window_len = playback_window.prefetch_indices.len();
        let repeat_mode = *self.repeat_mode.lock();

        let start_idx = *playback_window.prefetch_indices.first()?;

        if let Some(next_idx) = self.get_next_in_playback_order(start_idx, window_len, repeat_mode)
        {
            playback_window.prefetch_indices.push(next_idx);
            Some(next_idx)
        } else {
            None
        }
    }

    /// Get the next queue index in playback order at a given offset.
    ///
    /// - In sequential mode: returns start_idx + offset (with optional wrap)
    /// - In shuffle mode: finds start_idx in shuffle_order, returns
    ///   shuffle_order[pos + offset]
    fn get_next_in_playback_order(
        &self,
        start_idx: usize,
        offset: usize,
        repeat_mode: RepeatMode,
    ) -> Option<usize> {
        let len = self.len();
        if len == 0 {
            return None;
        }

        let shuffle_order = self.shuffle_order.lock();

        if let Some(ref order) = *shuffle_order {
            // Shuffle mode: find position of start_idx in shuffle order
            let start_pos = order.iter().position(|&idx| idx == start_idx)?;
            let target_pos = start_pos + offset;

            if target_pos < order.len() {
                Some(order[target_pos])
            } else if repeat_mode == RepeatMode::All {
                // Wrap around
                Some(order[target_pos % order.len()])
            } else {
                None
            }
        } else {
            // Sequential mode
            let target_idx = start_idx + offset;

            if target_idx < len {
                Some(target_idx)
            } else if repeat_mode == RepeatMode::All {
                Some(target_idx % len)
            } else {
                None
            }
        }
    }

    /// Generate a shuffle order with Fisher-Yates algorithm.
    ///
    /// Places the current track at the start so it doesn't immediately replay.
    fn generate_shuffle_order_internal(&self, len: usize, current: Option<usize>) -> Vec<usize> {
        use rand::Rng;

        let mut order: Vec<usize> = (0..len).collect();
        let mut rng = rand::thread_rng();

        // Fisher-Yates shuffle
        for i in (1..len).rev() {
            let j = rng.gen_range(0..=i);
            order.swap(i, j);
        }

        // Move current track to front if specified
        if let Some(curr) = current {
            if let Some(pos) = order.iter().position(|&x| x == curr) {
                order.swap(0, pos);
            }
        }

        order
    }

    /// Clear the prefetch tracking (called when rebuilding MPV playlist)
    pub fn clear_prefetch(&self) {
        self.playback_window.lock().prefetch_indices.clear();
    }

    /// Regenerate shuffle order (called when queue changes significantly)
    pub fn regenerate_shuffle_order(&self) {
        if *self.shuffle_enabled.lock() {
            let len = self.len();
            let current = self.current_index();
            *self.shuffle_order.lock() = Some(self.generate_shuffle_order_internal(len, current));
        }
    }
}

// ========== Unit Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_song(title: &str) -> Song {
        let mut song = Song::default();
        song.uri = format!("video_{}", title);
        song.metadata.insert("title".into(), vec![title.to_string()]);
        song
    }

    // ========== Basic Queue Operations ==========

    #[test]
    fn test_add_to_queue() {
        let queue = QueueService::new();
        let song = create_test_song("test");

        let id = queue.add(song, None);
        assert_eq!(id, 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_add_with_position() {
        let queue = QueueService::new();

        queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        queue.add(create_test_song("inserted"), Some(1));

        assert_eq!(queue.len(), 3);
        let song = queue.get_by_index(1).unwrap();
        assert_eq!(song.metadata.get("title").unwrap()[0], "inserted");
    }

    #[test]
    fn test_add_before_current_shifts_current_and_playback_base() {
        let queue = QueueService::new();

        queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        queue.add(create_test_song("third"), None);
        queue.set_current(Some(1));
        queue.set_playback_base_index(1);

        queue.add(create_test_song("inserted"), Some(0));

        assert_eq!(queue.current_index(), Some(2));
        assert_eq!(queue.playback_base_index(), 2);
    }

    #[test]
    fn test_remove_from_queue() {
        let queue = QueueService::new();
        let id = queue.add(create_test_song("test"), None);

        let result = queue.remove(id);
        assert!(result.is_ok());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);

        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.current_index().is_none());
    }

    #[test]
    fn test_get_by_id() {
        let queue = QueueService::new();
        let id = queue.add(create_test_song("test"), None);

        let song = queue.get_by_id(id).unwrap();
        assert_eq!(song.metadata.get("title").unwrap()[0], "test");
    }

    #[test]
    fn test_get_all() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);

        let all = queue.get_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_is_empty() {
        let queue = QueueService::new();
        assert!(queue.is_empty());

        queue.add(create_test_song("test"), None);
        assert!(!queue.is_empty());
    }

    // ========== MPV Sync: Remove Returns Index ==========

    #[test]
    fn test_remove_returns_index_for_mpv_sync() {
        let queue = QueueService::new();
        queue.add(create_test_song("first"), None);
        let id2 = queue.add(create_test_song("second"), None);
        queue.add(create_test_song("third"), None);

        // Remove middle element - should return index 1
        let result = queue.remove(id2);
        assert!(result.is_ok());

        let (removed_item, mpv_index) = result.unwrap();
        assert_eq!(mpv_index, 1); // Was at index 1
        assert_eq!(removed_item.song.metadata.get("title").unwrap()[0], "second");
    }

    #[test]
    fn test_remove_first_returns_index_zero() {
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);

        let (_, mpv_index) = queue.remove(id1).unwrap();
        assert_eq!(mpv_index, 0);
    }

    // ========== Regression: Delete Playing Song (Zombie Fix) ==========

    #[test]
    fn test_remove_playing_song_clears_current_index() {
        let queue = QueueService::new();
        queue.add(create_test_song("first"), None);
        let id2 = queue.add(create_test_song("second"), None);
        queue.add(create_test_song("third"), None);

        // Simulate playing second song
        queue.set_current(Some(1));
        assert_eq!(queue.current_index(), Some(1));

        // Delete the playing song
        let result = queue.remove(id2);
        assert!(result.is_ok());

        // Current index should be cleared (MPV will stop)
        assert_eq!(queue.current_index(), None);
    }

    #[test]
    fn test_remove_before_current_shifts_index() {
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        queue.add(create_test_song("third"), None);

        // Playing third song (index 2)
        queue.set_current(Some(2));

        // Remove first song
        queue.remove(id1).unwrap();

        // Current should shift down to 1
        assert_eq!(queue.current_index(), Some(1));
    }

    // ========== MPV Sync: Move Returns Indices ==========

    #[test]
    fn test_move_forward_places_at_target_position() {
        // Queue: [A, B, C] → Move A to C's position → [B, A, C]
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        let id3 = queue.add(create_test_song("third"), None);

        // Move first to third position
        let result = queue.move_song(id1, id3);
        assert!(result.is_ok());

        let (from_idx, to_idx) = result.unwrap();
        assert_eq!(from_idx, 0); // Was at index 0
        // After removal, queue is [B, C]. Target was 2, adjusted to 1.
        assert_eq!(to_idx, 1);

        // Verify queue order: [B, A, C]
        let all = queue.get_all();
        assert_eq!(all[0].metadata.get("title").unwrap()[0], "second");
        assert_eq!(all[1].metadata.get("title").unwrap()[0], "first");
        assert_eq!(all[2].metadata.get("title").unwrap()[0], "third");
    }

    #[test]
    fn test_move_backward_places_at_target_position() {
        // Queue: [A, B, C] → Move C to A's position → [C, A, B]
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        let id3 = queue.add(create_test_song("third"), None);

        // Move third to first position
        let result = queue.move_song(id3, id1);
        assert!(result.is_ok());

        let (from_idx, to_idx) = result.unwrap();
        assert_eq!(from_idx, 2); // Was at index 2
        assert_eq!(to_idx, 0); // Now at index 0 (no adjustment needed when from > to)

        // Verify queue order: [C, A, B]
        let all = queue.get_all();
        assert_eq!(all[0].metadata.get("title").unwrap()[0], "third");
        assert_eq!(all[1].metadata.get("title").unwrap()[0], "first");
        assert_eq!(all[2].metadata.get("title").unwrap()[0], "second");
    }

    #[test]
    fn test_move_to_same_position_is_noop() {
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);

        let result = queue.move_song(id1, id1);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), (0, 0));
    }

    #[test]
    fn test_move_playing_song_updates_current_index() {
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        let id3 = queue.add(create_test_song("third"), None);

        // Playing first song
        queue.set_current(Some(0));

        // Move first song to third's position
        // Queue: [A*, B, C] → [B, A*, C]
        queue.move_song(id1, id3).unwrap();

        // Current should follow the moved track (now at index 1)
        assert_eq!(queue.current_index(), Some(1));
    }

    // ========== find_index_by_id ==========

    #[test]
    fn test_find_index_by_id() {
        let queue = QueueService::new();
        queue.add(create_test_song("first"), None);
        let id2 = queue.add(create_test_song("second"), None);
        queue.add(create_test_song("third"), None);

        assert_eq!(queue.find_index_by_id(id2), Some(1));
    }

    #[test]
    fn test_find_index_by_id_not_found() {
        let queue = QueueService::new();
        queue.add(create_test_song("first"), None);

        assert_eq!(queue.find_index_by_id(999), None);
    }

    // ========== Edge Cases ==========

    #[test]
    fn test_remove_nonexistent_returns_error() {
        let queue = QueueService::new();
        queue.add(create_test_song("test"), None);

        let result = queue.remove(999);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_assigns_sequential_ids() {
        let queue = QueueService::new();

        let id1 = queue.add(create_test_song("one"), None);
        let id2 = queue.add(create_test_song("two"), None);
        let id3 = queue.add(create_test_song("three"), None);

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_add_sets_song_id() {
        let queue = QueueService::new();
        let id = queue.add(create_test_song("test"), None);

        let song = queue.get_by_id(id).unwrap();
        assert_eq!(song.id, Some(id));
    }

    #[test]
    fn compute_prefetch_window_is_pure() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        queue.add(create_test_song("three"), None);

        let indices = queue.compute_prefetch_window(0, 3);

        assert_eq!(indices, vec![0, 1, 2]);
        assert_eq!(queue.get_prefetched_at(0), None);
    }

    #[test]
    fn compute_background_extract_scope_rotates_full_queue() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        queue.add(create_test_song("three"), None);

        assert_eq!(queue.compute_background_extract_scope(1), vec![1, 2, 0]);
    }

    #[test]
    fn set_playback_window_state_updates_related_fields_together() {
        let queue = QueueService::new();

        queue.set_playback_window_state(Some(2), 1, vec![1, 2, 3]);

        assert_eq!(
            queue.playback_window_state(),
            PlaybackWindowState {
                current_idx: Some(2),
                playback_base_index: 1,
                prefetch_indices: vec![1, 2, 3],
            }
        );
    }
}
