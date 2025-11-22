use serde::{Deserialize, Serialize};

/// Queue position specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePosition {
    /// Absolute position in queue (0-based)
    Absolute(usize),
    
    /// Relative to current position (positive = after, negative = before)
    Relative(i32),
    
    /// At the end of the queue
    End,
    
    /// After the current song
    Next,
}

impl QueuePosition {
    /// Convert to absolute position given current queue length and position
    pub fn to_absolute(&self, current_pos: Option<usize>, queue_len: usize) -> Option<usize> {
        match self {
            QueuePosition::Absolute(pos) => Some(*pos),
            QueuePosition::Relative(offset) => {
                current_pos.and_then(|pos| {
                    let new_pos = pos as i32 + offset;
                    if new_pos >= 0 && (new_pos as usize) < queue_len {
                        Some(new_pos as usize)
                    } else {
                        None
                    }
                })
            }
            QueuePosition::End => Some(queue_len),
            QueuePosition::Next => current_pos.map(|pos| pos + 1),
        }
    }
}
