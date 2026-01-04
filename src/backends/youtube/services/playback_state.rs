use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Loaded,
    Playing,
    EndOfFile,
    Stopped,
    Paused,
}

#[derive(Debug, Clone)]
pub struct PlaybackStateTracker {
    state: Arc<Mutex<PlaybackState>>,
}

impl Default for PlaybackStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackStateTracker {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(PlaybackState::Idle)) }
    }

    pub fn get(&self) -> PlaybackState {
        *self.state.lock().unwrap()
    }

    pub fn transition(&self, from: PlaybackState, to: PlaybackState) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == from {
            log::debug!("State transition: {:?} -> {:?}", from, to);
            *state = to;
            true
        } else {
            log::warn!("Invalid transition: expected {:?}, got {:?}", from, *state);
            false
        }
    }

    pub fn force_set(&self, to: PlaybackState) {
        *self.state.lock().unwrap() = to;
    }
}
