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
            QueuePosition::Relative(offset) => current_pos.and_then(|pos| {
                let new_pos = pos as i32 + offset;
                if new_pos >= 0 && (new_pos as usize) < queue_len {
                    Some(new_pos as usize)
                } else {
                    None
                }
            }),
            QueuePosition::End => Some(queue_len),
            QueuePosition::Next => current_pos.map(|pos| pos + 1),
        }
    }
}

/// Conversion from domain QueuePosition to MPD QueuePosition
impl From<QueuePosition> for crate::mpd::queue_position::QueuePosition {
    fn from(pos: QueuePosition) -> Self {
        match pos {
            QueuePosition::Absolute(n) => crate::mpd::queue_position::QueuePosition::Absolute(n),
            QueuePosition::Relative(offset) if offset >= 0 => {
                crate::mpd::queue_position::QueuePosition::RelativeAdd(offset as usize)
            }
            QueuePosition::Relative(offset) => {
                crate::mpd::queue_position::QueuePosition::RelativeSub(offset.abs() as usize)
            }
            QueuePosition::End => crate::mpd::queue_position::QueuePosition::Absolute(usize::MAX),
            QueuePosition::Next => crate::mpd::queue_position::QueuePosition::RelativeAdd(1),
        }
    }
}

impl From<crate::mpd::QueuePosition> for QueuePosition {
    fn from(pos: crate::mpd::QueuePosition) -> Self {
        match pos {
            crate::mpd::QueuePosition::Absolute(idx) => QueuePosition::Absolute(idx),
            crate::mpd::QueuePosition::RelativeAdd(offset) => {
                QueuePosition::Relative(offset as i32)
            }
            crate::mpd::QueuePosition::RelativeSub(offset) => {
                QueuePosition::Relative(-(offset as i32))
            }
        }
    }
}
