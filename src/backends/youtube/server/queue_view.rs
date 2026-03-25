use parking_lot::Mutex;

use crate::{
    backends::youtube::services::{QueueService, RepeatMode as ServiceRepeatMode},
    domain::Song,
    shared::play_queue::{PlayQueue, RepeatMode as PlayQueueRepeatMode},
};

pub trait QueueView {
    fn len(&self) -> usize;
    fn current_index(&self) -> Option<usize>;
    fn get_by_index(&self, idx: usize) -> Option<Song>;
    fn get_all(&self) -> Vec<Song>;
    fn repeat_label(&self) -> &'static str;
    fn shuffle_enabled(&self) -> bool;
    fn next_index(&self) -> Option<usize>;
}

impl QueueView for QueueService {
    fn len(&self) -> usize {
        self.len()
    }

    fn current_index(&self) -> Option<usize> {
        self.current_index()
    }

    fn get_by_index(&self, idx: usize) -> Option<Song> {
        self.get_by_index(idx).ok()
    }

    fn get_all(&self) -> Vec<Song> {
        self.get_all()
    }

    fn repeat_label(&self) -> &'static str {
        match self.repeat_mode() {
            ServiceRepeatMode::Off => "off",
            ServiceRepeatMode::One => "one",
            ServiceRepeatMode::All => "all",
        }
    }

    fn shuffle_enabled(&self) -> bool {
        self.shuffle_enabled()
    }

    fn next_index(&self) -> Option<usize> {
        self.next_index()
    }
}

impl QueueView for Mutex<PlayQueue> {
    fn len(&self) -> usize {
        self.lock().len()
    }

    fn current_index(&self) -> Option<usize> {
        self.lock().get_current_position()
    }

    fn get_by_index(&self, idx: usize) -> Option<Song> {
        let queue = self.lock();
        let id = *queue.get_play_order().get(idx)?;
        queue.get_song(id).cloned()
    }

    fn get_all(&self) -> Vec<Song> {
        self.lock().get_songs_in_order().into_iter().cloned().collect()
    }

    fn repeat_label(&self) -> &'static str {
        let mode = self.lock().get_repeat();
        match mode {
            PlayQueueRepeatMode::Off => "off",
            PlayQueueRepeatMode::One => "one",
            PlayQueueRepeatMode::All => "all",
        }
    }

    fn shuffle_enabled(&self) -> bool {
        self.lock().get_shuffle()
    }

    fn next_index(&self) -> Option<usize> {
        let queue = self.lock();
        let len = queue.len();
        let current = queue.get_current_position();
        if len == 0 {
            return None;
        }

        match current {
            Some(idx) if idx + 1 < len => Some(idx + 1),
            Some(_) if queue.get_repeat() == PlayQueueRepeatMode::All => Some(0),
            _ => None,
        }
    }
}
