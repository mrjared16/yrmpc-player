#![allow(clippy::doc_markdown, clippy::must_use_candidate)]

//! TogglePlaybackHandler - handles play/pause toggle.
//!
//! ## Responsibility (SRP)
//!
//! This handler has a single responsibility: toggle play/pause on the current
//! playback. It does NOT:
//! - Play new content (that's PlayHandler)
//! - Manage queue (that's QueueHandler)
//!
//! ## Usage
//!
//! ```ignore
//! let intent = Intent::toggle_playback();
//! dispatcher.dispatch(intent, ctx);
//! ```

use anyhow::Result;

use crate::{
    actions::{
        handler::{HandleResult, Handler},
        intent::{Intent, IntentKind},
    },
    ctx::Ctx,
};

/// Handles TogglePlayback action.
#[derive(Debug, Default)]
pub struct TogglePlaybackHandler;

impl TogglePlaybackHandler {
    /// Create a new TogglePlaybackHandler.
    pub fn new() -> Self {
        Self
    }
}

impl Handler for TogglePlaybackHandler {
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        // Only handle TogglePlayback action
        if intent.action != IntentKind::TogglePlayback {
            return Ok(HandleResult::Skip);
        }

        // Toggle play/pause
        ctx.command(|client| {
            client.pause_toggle()?;
            Ok(())
        });

        Ok(HandleResult::Done)
    }

    fn priority(&self) -> i32 {
        10 // Higher priority - simple operation
    }

    fn name(&self) -> &'static str {
        "TogglePlaybackHandler"
    }
}
