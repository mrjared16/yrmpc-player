//! QueueHandler - handles queue operations (add, remove, move).

use anyhow::Result;

use crate::{
    actions::{
        handler::{HandleResult, Handler},
        intent::{Intent, IntentKind},
    },
    backends::youtube::protocol::play_intent::PlayIntent,
    ctx::Ctx,
    domain::ContentType,
};

/// Handles queue operations.
#[derive(Debug, Default)]
pub struct QueueHandler;

impl QueueHandler {
    /// Create a new QueueHandler.
    pub fn new() -> Self {
        Self
    }
}

impl Handler for QueueHandler {
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        match intent.action {
            IntentKind::AddToQueue => {
                if intent.selection.is_empty() {
                    return Ok(HandleResult::NotApplicable("Select something to add"));
                }

                if !intent.selection.has_only(&[
                    ContentType::Track,
                    ContentType::Album,
                    ContentType::Playlist,
                ]) {
                    return Ok(HandleResult::NotApplicable("Cannot add this to queue"));
                }

                let songs = intent.selection.songs_cloned();
                if songs.is_empty() {
                    return Ok(HandleResult::NotApplicable("No songs to add"));
                }
                ctx.queue_store().play(PlayIntent::Append { tracks: songs });
                Ok(HandleResult::Done)
            }

            IntentKind::RemoveFromQueue => {
                let ids: Vec<u32> =
                    intent.selection.songs_cloned().iter().filter_map(|s| s.id).collect();
                if ids.is_empty() {
                    return Ok(HandleResult::NotApplicable("No songs to remove"));
                }
                ctx.queue_store().remove_ids(&ids);
                Ok(HandleResult::Done)
            }

            IntentKind::MoveUp | IntentKind::MoveDown => Ok(HandleResult::Skip),

            _ => Ok(HandleResult::Skip),
        }
    }

    fn priority(&self) -> i32 {
        0
    }

    fn name(&self) -> &'static str {
        "QueueHandler"
    }
}
