//! `PlayQueue` - Pure state machine for queue management
//!
//! This is Layer 1 of the queue architecture: a pure, event-emitting state
//! machine.
//! - NO async code, NO I/O, NO external dependencies
//! - Commands in, events out
//! - All state transitions are deterministic and testable
//!
//! Layer 2 (Bridge) will consume events and sync with MPV.

use std::collections::{HashMap, VecDeque};

use rand::seq::SliceRandom;

use crate::domain::Song;

/// Unique identifier for queue items
pub type QueueId = u64;

/// Repeat mode for the queue
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    /// No repeat - stop at end of queue
    Off,
    /// Repeat entire queue (loop back to start)
    All,
    /// Repeat current track
    One,
}

/// Commands that can be applied to the queue
#[derive(Debug, Clone)]
pub enum QueueCommand {
    /// Add a single song to the end of the queue
    Add { song: Song },
    AddAt { song: Song, position: usize },
    /// Add multiple songs to the end of the queue
    AddBatch { songs: Vec<Song> },
    /// Remove a song by its ID
    Remove { id: QueueId },
    /// Move a song to a new position
    Move { id: QueueId, to_position: usize },
    /// Clear the entire queue
    Clear,
    /// Play a specific song by ID
    Play { id: QueueId },
    /// Move to next song
    Next,
    /// Move to previous song
    Previous,
    /// Enable/disable shuffle
    SetShuffle { enabled: bool },
    /// Set repeat mode
    SetRepeat { mode: RepeatMode },
    /// Advance to next track (called by bridge on track end)
    Advance,
    /// Stop playback
    Stop,
}

/// Events emitted by the queue
#[derive(Debug, Clone)]
pub enum QueueEvent {
    /// Items were added to the queue
    ItemsAdded { ids: Vec<QueueId> },
    /// Items were removed from the queue
    ItemsRemoved { ids: Vec<QueueId> },
    /// The play order changed (due to shuffle/unshuffle/move)
    OrderChanged { play_order: Vec<QueueId>, current_id: Option<QueueId> },
    /// The current playing track changed
    CurrentChanged { from: Option<QueueId>, to: Option<QueueId> },
    /// Shuffle or repeat mode changed
    ModesChanged { shuffle: bool, repeat: RepeatMode },
    /// Queue was cleared
    Cleared,
    /// Playback stopped
    Stopped,
}

/// Pure state machine for queue management
#[derive(Debug, Clone)]
pub struct PlayQueue {
    /// All queue items indexed by ID
    items: HashMap<QueueId, Song>,
    /// Original insertion order (never shuffled)
    original_order: Vec<QueueId>,
    /// Current play order (shuffled if shuffle is enabled)
    play_order: Vec<QueueId>,
    /// Currently playing item ID
    current_id: Option<QueueId>,
    /// History of played items (for "previous" in shuffle mode)
    history: VecDeque<QueueId>,

    /// Shuffle enabled
    shuffle: bool,
    /// Repeat mode
    repeat: RepeatMode,

    /// Next ID to assign
    next_id: QueueId,
    /// Maximum history size
    history_limit: usize,
}

impl PlayQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            original_order: Vec::new(),
            play_order: Vec::new(),
            current_id: None,
            history: VecDeque::new(),
            shuffle: false,
            repeat: RepeatMode::Off,
            next_id: 1,
            history_limit: 100,
        }
    }

    /// Apply a command and return emitted events
    pub fn apply(&mut self, cmd: QueueCommand) -> Vec<QueueEvent> {
        match cmd {
            QueueCommand::Add { song } => self.handle_add(song),
            QueueCommand::AddAt { song, position } => self.handle_add_at(song, position),
            QueueCommand::AddBatch { songs } => self.handle_add_batch(songs),
            QueueCommand::Remove { id } => self.handle_remove(id),
            QueueCommand::Move { id, to_position } => self.handle_move(id, to_position),
            QueueCommand::Clear => self.handle_clear(),
            QueueCommand::Play { id } => self.handle_play(id),
            QueueCommand::Next => self.handle_next(),
            QueueCommand::Previous => self.handle_previous(),
            QueueCommand::SetShuffle { enabled } => self.handle_set_shuffle(enabled),
            QueueCommand::SetRepeat { mode } => self.handle_set_repeat(mode),
            QueueCommand::Advance => self.handle_advance(),
            QueueCommand::Stop => self.handle_stop(),
        }
    }

    // =========================================================================
    // Command Handlers
    // =========================================================================

    fn handle_add(&mut self, song: Song) -> Vec<QueueEvent> {
        self.handle_add_at(song, self.play_order.len())
    }

    fn handle_add_at(&mut self, song: Song, position: usize) -> Vec<QueueEvent> {
        let id = self.next_id;
        self.next_id += 1;

        self.items.insert(id, song);
        let insert_pos = position.min(self.play_order.len());
        self.original_order.insert(insert_pos.min(self.original_order.len()), id);
        self.play_order.insert(insert_pos, id);

        vec![QueueEvent::ItemsAdded { ids: vec![id] }]
    }

    fn handle_add_batch(&mut self, songs: Vec<Song>) -> Vec<QueueEvent> {
        let mut ids = Vec::with_capacity(songs.len());

        for song in songs {
            let id = self.next_id;
            self.next_id += 1;

            self.items.insert(id, song);
            self.original_order.push(id);
            self.play_order.push(id);
            ids.push(id);
        }

        vec![QueueEvent::ItemsAdded { ids }]
    }

    fn handle_remove(&mut self, id: QueueId) -> Vec<QueueEvent> {
        if !self.items.contains_key(&id) {
            return vec![];
        }

        self.items.remove(&id);
        self.original_order.retain(|&x| x != id);
        self.play_order.retain(|&x| x != id);
        self.history.retain(|&x| x != id);

        let mut events = vec![QueueEvent::ItemsRemoved { ids: vec![id] }];

        // If we removed the current track, stop
        if self.current_id == Some(id) {
            let from = self.current_id;
            self.current_id = None;
            events.push(QueueEvent::CurrentChanged { from, to: None });
        }

        events
    }

    fn handle_move(&mut self, id: QueueId, to_position: usize) -> Vec<QueueEvent> {
        // Find current position in play_order
        let from_pos = match self.play_order.iter().position(|&x| x == id) {
            Some(pos) => pos,
            None => return vec![], // ID not found
        };

        let to_pos = to_position.min(self.play_order.len().saturating_sub(1));

        if from_pos == to_pos {
            return vec![]; // No-op
        }

        // Remove and reinsert
        self.play_order.remove(from_pos);
        self.play_order.insert(to_pos, id);

        // Also update original_order to match
        if let Some(orig_pos) = self.original_order.iter().position(|&x| x == id) {
            self.original_order.remove(orig_pos);
            let orig_to = to_pos.min(self.original_order.len());
            self.original_order.insert(orig_to, id);
        }

        vec![QueueEvent::OrderChanged {
            play_order: self.play_order.clone(),
            current_id: self.current_id,
        }]
    }

    fn handle_clear(&mut self) -> Vec<QueueEvent> {
        self.items.clear();
        self.original_order.clear();
        self.play_order.clear();
        self.current_id = None;
        self.history.clear();

        vec![QueueEvent::Cleared]
    }

    fn handle_play(&mut self, id: QueueId) -> Vec<QueueEvent> {
        if !self.items.contains_key(&id) {
            return vec![];
        }

        let from = self.current_id;
        self.current_id = Some(id);

        // Add previous track to history
        if let Some(prev) = from {
            self.add_to_history(prev);
        }

        vec![QueueEvent::CurrentChanged { from, to: Some(id) }]
    }

    fn handle_next(&mut self) -> Vec<QueueEvent> {
        let from = self.current_id;

        // Add current to history before moving
        if let Some(current) = self.current_id {
            self.add_to_history(current);
        }

        let next = self.get_next_id();
        self.current_id = next;

        vec![QueueEvent::CurrentChanged { from, to: next }]
    }

    fn handle_previous(&mut self) -> Vec<QueueEvent> {
        let from = self.current_id;

        // In shuffle mode, go back through history
        let prev = if self.shuffle {
            self.get_previous_from_history()
        } else {
            self.get_previous_sequential()
        };

        self.current_id = prev;

        vec![QueueEvent::CurrentChanged { from, to: prev }]
    }

    fn handle_set_shuffle(&mut self, enabled: bool) -> Vec<QueueEvent> {
        if enabled == self.shuffle {
            return vec![];
        }

        self.shuffle = enabled;

        if enabled {
            // Shuffle: current song stays first, rest randomized
            let mut rng = rand::thread_rng();
            let current_pos =
                self.current_id.and_then(|id| self.play_order.iter().position(|&x| x == id));

            self.play_order = self.original_order.clone();
            if let Some(pos) = current_pos {
                self.play_order.swap(0, pos);
            }
            self.play_order[1..].shuffle(&mut rng);
        } else {
            // Unshuffle: restore original order
            self.play_order = self.original_order.clone();
        }

        vec![
            QueueEvent::OrderChanged {
                play_order: self.play_order.clone(),
                current_id: self.current_id,
            },
            QueueEvent::ModesChanged { shuffle: self.shuffle, repeat: self.repeat },
        ]
    }

    fn handle_set_repeat(&mut self, mode: RepeatMode) -> Vec<QueueEvent> {
        if mode == self.repeat {
            return vec![];
        }

        self.repeat = mode;

        vec![QueueEvent::ModesChanged { shuffle: self.shuffle, repeat: self.repeat }]
    }

    fn handle_advance(&mut self) -> Vec<QueueEvent> {
        // Same as Next but respects RepeatMode::One
        if self.repeat == RepeatMode::One {
            // Don't advance, stay on current
            return vec![];
        }

        self.handle_next()
    }

    fn handle_stop(&mut self) -> Vec<QueueEvent> {
        let from = self.current_id;
        self.current_id = None;

        if from.is_some() {
            vec![QueueEvent::CurrentChanged { from, to: None }, QueueEvent::Stopped]
        } else {
            vec![QueueEvent::Stopped]
        }
    }

    // =========================================================================
    // Internal Helpers
    // =========================================================================

    fn get_next_id(&self) -> Option<QueueId> {
        let current_pos =
            self.current_id.and_then(|id| self.play_order.iter().position(|&x| x == id));

        match current_pos {
            Some(pos) if pos + 1 < self.play_order.len() => Some(self.play_order[pos + 1]),
            Some(_) if self.repeat == RepeatMode::All => self.play_order.first().copied(),
            _ => None,
        }
    }

    fn get_previous_sequential(&self) -> Option<QueueId> {
        let current_pos =
            self.current_id.and_then(|id| self.play_order.iter().position(|&x| x == id));

        match current_pos {
            Some(pos) if pos > 0 => Some(self.play_order[pos - 1]),
            Some(0) if self.repeat == RepeatMode::All => self.play_order.last().copied(),
            _ => None,
        }
    }

    fn get_previous_from_history(&mut self) -> Option<QueueId> {
        // Return the last played song from history
        self.history.pop_back()
    }

    fn add_to_history(&mut self, id: QueueId) {
        // Avoid duplicates at end
        if self.history.back() != Some(&id) {
            self.history.push_back(id);
            while self.history.len() > self.history_limit {
                self.history.pop_front();
            }
        }
    }

    // =========================================================================
    // Public Read Methods (for TUI and Bridge)
    // =========================================================================

    #[must_use]
    pub fn get_play_order(&self) -> &[QueueId] {
        &self.play_order
    }

    #[must_use]
    pub fn get_current_id(&self) -> Option<QueueId> {
        self.current_id
    }

    #[must_use]
    pub fn get_song(&self, id: QueueId) -> Option<&Song> {
        self.items.get(&id)
    }

    #[must_use]
    pub fn get_songs_in_order(&self) -> Vec<&Song> {
        self.play_order.iter().filter_map(|id| self.items.get(id)).collect()
    }

    #[must_use]
    pub fn get_shuffle(&self) -> bool {
        self.shuffle
    }

    #[must_use]
    pub fn get_repeat(&self) -> RepeatMode {
        self.repeat
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn get_original_order(&self) -> &[QueueId] {
        &self.original_order
    }

    #[must_use]
    pub fn get_position(&self, id: QueueId) -> Option<usize> {
        self.play_order.iter().position(|&x| x == id)
    }

    #[must_use]
    pub fn get_current_position(&self) -> Option<usize> {
        self.current_id.and_then(|id| self.get_position(id))
    }
}

impl Default for PlayQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
