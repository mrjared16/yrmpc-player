//! QueueHandler - handles queue operations (add, remove, move).

use anyhow::Result;

use crate::{
    actions::{
        intent::{IntentKind, Intent},
        handler::{HandleResult, Handler},
    },
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
                for song in songs {
                    let uri = song.uri.clone();
                    ctx.command(move |client| {
                        client.add(&uri, None)?;
                        Ok(())
                    });
                }

                Ok(HandleResult::Done)
            }

            IntentKind::RemoveFromQueue => {
                let songs = intent.selection.songs_cloned();
                for song in songs {
                    if let Some(id) = song.id {
                        ctx.command(move |client| {
                            client.delete_id(id)?;
                            Ok(())
                        });
                    }
                }
                Ok(HandleResult::Done)
            }

            IntentKind::MoveUp | IntentKind::MoveDown => {
                // Queue move operations - handled by Navigator for now
                // as they need position calculations
                Ok(HandleResult::Skip)
            }

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
