use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Intent captured at EOF time, determines what should happen when MPV confirms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvanceIntent {
    /// Normal advance to next track in queue
    Advance,
    /// RepeatOne: seek to 0, stay on current track
    Repeat,
    /// End of queue, no repeat: stop playback
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Loaded,
    Playing,
    PendingAdvance { since: Instant, from_position: usize, intent: AdvanceIntent },
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
        let is_from_match = match from {
            PlaybackState::PendingAdvance { .. } => {
                matches!(*state, PlaybackState::PendingAdvance { .. })
            }
            _ => *state == from,
        };

        if is_from_match {
            log::debug!("State transition: {:?} -> {:?}", *state, to);
            *state = to;
            true
        } else {
            log::warn!("Invalid transition: expected {:?}, got {:?}", from, *state);
            false
        }
    }

    pub fn is_pending_expired(&self, timeout: Duration) -> bool {
        match *self.state.lock().unwrap() {
            PlaybackState::PendingAdvance { since, .. } => since.elapsed() > timeout,
            _ => false,
        }
    }

    pub fn force_set(&self, to: PlaybackState) {
        *self.state.lock().unwrap() = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_advance_transition() {
        let tracker = PlaybackStateTracker::new();
        tracker.force_set(PlaybackState::Playing);

        let to = PlaybackState::PendingAdvance {
            since: Instant::now(),
            from_position: 7,
            intent: AdvanceIntent::Advance,
        };
        assert!(tracker.transition(PlaybackState::Playing, to));

        let current = tracker.get();
        assert!(matches!(current, PlaybackState::PendingAdvance { from_position: 7, .. }));

        assert!(tracker.transition(
            PlaybackState::PendingAdvance {
                since: Instant::now(),
                from_position: 0,
                intent: AdvanceIntent::Advance,
            },
            PlaybackState::Playing
        ));

        tracker.force_set(PlaybackState::PendingAdvance {
            since: Instant::now(),
            from_position: 1,
            intent: AdvanceIntent::Advance,
        });
        assert!(tracker.transition(
            PlaybackState::PendingAdvance {
                since: Instant::now(),
                from_position: 0,
                intent: AdvanceIntent::Advance,
            },
            PlaybackState::Idle
        ));
    }

    #[test]
    fn test_pending_advance_timeout() {
        let tracker = PlaybackStateTracker::new();
        let timeout = Duration::from_millis(50);

        tracker.force_set(PlaybackState::PendingAdvance {
            since: Instant::now(),
            from_position: 0,
            intent: AdvanceIntent::Advance,
        });
        assert!(!tracker.is_pending_expired(timeout));

        tracker.force_set(PlaybackState::PendingAdvance {
            since: Instant::now() - (timeout + Duration::from_millis(1)),
            from_position: 0,
            intent: AdvanceIntent::Advance,
        });
        assert!(tracker.is_pending_expired(timeout));

        tracker.force_set(PlaybackState::Playing);
        assert!(!tracker.is_pending_expired(timeout));
    }
}
