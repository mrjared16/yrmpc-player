//! Interactive Item Actions
//!
//! Extends the existing domain/search action system with context-aware
//! execution for queue operations.
//!
//! This integrates with the existing ItemAction/QueueAction enums.

use anyhow::{Result, anyhow};
use crate::ctx::Ctx;
use crate::domain::Song;

/// Context in which an item is displayed
/// Affects which actions are available
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemContext {
    /// Item is in the playback queue
    Queue,
    /// Item is in search results
    SearchResult,
    /// Item is in a playlist
    Playlist,
}

/// Queue-specific actions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemAction {
    /// Play this item now
    Play,
    /// Play if distinct, toggle pause if same song
    PlayOrToggle,
    /// Delete from queue
    Delete,
    /// Move up in queue
    MoveUp,
    /// Move down in queue
    MoveDown,
}

impl QueueItemAction {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::PlayOrToggle => "Play/Pause",
            Self::Delete => "Delete",
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
        }
    }
    
    pub fn shortcut(&self) -> Option<&'static str> {
        match self {
            Self::Play => Some("Enter"),
            Self::PlayOrToggle => Some("Enter"),
            Self::Delete => Some("d"),
            Self::MoveUp => Some("K"),
            Self::MoveDown => Some("J"),
        }
    }
}

/// Extension trait for Song to execute queue actions
pub trait QueueItemOps {
    /// Get available actions for this song in queue context
    fn queue_actions(&self) -> Vec<QueueItemAction>;
    
    /// Execute a queue action
    fn execute_queue_action(&self, action: QueueItemAction, ctx: &mut Ctx) -> Result<()>;
}

impl QueueItemOps for Song {
    fn queue_actions(&self) -> Vec<QueueItemAction> {
        vec![
            QueueItemAction::Play,
            QueueItemAction::Delete,
            QueueItemAction::MoveUp,
            QueueItemAction::MoveDown,
        ]
    }
    
    fn execute_queue_action(&self, action: QueueItemAction, ctx: &mut Ctx) -> Result<()> {
        match action {
            QueueItemAction::Play => {
                let id = self.id.unwrap_or_default();
                ctx.command(move |client| {
                    client.play_id(id)?;
                    Ok(())
                });
                Ok(())
            }
            QueueItemAction::PlayOrToggle => {
                let id = self.id.unwrap_or_default();
                let current_id = ctx.find_current_song_in_queue()
                    .and_then(|(_, s)| s.id);
                
                if current_id == Some(id) {
                    // Same song - toggle pause/resume
                    ctx.command(|client| Ok(client.pause_toggle()?));
                } else {
                    // Different song - play from start (seek to 0)
                    ctx.command(move |client| {
                        client.play_id(id)?;
                        // Seek to start to avoid resume-from-last-position
                        client.seek_current(crate::mpd::commands::SeekPosition::Absolute(0.0))?;
                        Ok(())
                    });
                }
                Ok(())
            }
            QueueItemAction::Delete => {
                let id = self.id.unwrap_or_default();
                // Use query (not command) to return updated queue for instant UI refresh
                ctx.query()
                    .id("queue_delete_action")
                    .query(move |client| {
                        client.delete_id(id)?;
                        // Return updated queue - event loop auto-updates ctx.queue
                        let queue = client.playlist_info()?;
                        Ok(crate::QueryResult::Queue(Some(queue)))
                    });
                Ok(())
            }
            QueueItemAction::MoveUp => {
                let id = self.id.unwrap_or_default();
                // Find current index in queue
                let current_idx = ctx.queue.iter().position(|s| s.id == Some(id));

                if let Some(idx) = current_idx {
                    if idx > 0 {
                        let new_idx = idx - 1;
                        // Use query for instant UI refresh
                        ctx.query()
                            .id("queue_move_action")
                            .query(move |client| {
                                client.move_id(id, new_idx as u32)?;
                                let queue = client.playlist_info()?;
                                Ok(crate::QueryResult::Queue(Some(queue)))
                            });
                    }
                }
                Ok(())
            }
            QueueItemAction::MoveDown => {
                let id = self.id.unwrap_or_default();
                // Find current index in queue
                let current_idx = ctx.queue.iter().position(|s| s.id == Some(id));

                if let Some(idx) = current_idx {
                    if idx < ctx.queue.len().saturating_sub(1) {
                        let new_idx = idx + 1;
                        // Use query for instant UI refresh
                        ctx.query()
                            .id("queue_move_action")
                            .query(move |client| {
                                client.move_id(id, new_idx as u32)?;
                                let queue = client.playlist_info()?;
                                Ok(crate::QueryResult::Queue(Some(queue)))
                            });
                    }
                }
                Ok(())
            }
        }
    }
}
