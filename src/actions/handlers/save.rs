//! SaveHandler - handles Save action for albums/playlists to library.

use anyhow::Result;

use crate::{
    actions::{
        handler::{HandleResult, Handler},
        intent::{Intent, IntentKind},
    },
    ctx::Ctx,
    domain::ContentType,
};

/// Handles Save actions (save to library).
#[derive(Debug, Default)]
pub struct SaveHandler;

impl SaveHandler {
    /// Create a new SaveHandler.
    pub fn new() -> Self {
        Self
    }
}

impl Handler for SaveHandler {
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        // Only handle Save action
        if intent.action != IntentKind::SaveToLibrary {
            return Ok(HandleResult::Skip);
        }

        if intent.selection.is_empty() {
            return Ok(HandleResult::NotApplicable("Select something to save"));
        }

        // Only albums and playlists can be saved to library
        if !intent.selection.has_only(&[ContentType::Album, ContentType::Playlist]) {
            return Ok(HandleResult::NotApplicable("Can only save albums or playlists"));
        }

        // TODO: Implement actual save to library logic
        // This requires backend support for library management
        // For now, return NotApplicable until backend implements save
        let _ = ctx; // Suppress unused warning
        Ok(HandleResult::NotApplicable("Save to library not yet implemented"))
    }

    fn priority(&self) -> i32 {
        0
    }

    fn name(&self) -> &'static str {
        "SaveHandler"
    }
}
