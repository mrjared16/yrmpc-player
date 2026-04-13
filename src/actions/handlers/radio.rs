#![allow(clippy::must_use_candidate)]

use anyhow::Result;

use crate::{
    actions::{
        handler::{HandleResult, Handler},
        intent::{Intent, IntentKind},
    },
    backends::youtube::protocol::play_intent::{MixType, PlayIntent},
    ctx::Ctx,
    domain::ContentType,
};

#[derive(Debug, Default)]
pub struct RadioHandler;

impl RadioHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Handler for RadioHandler {
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        if intent.action != IntentKind::StartRadio {
            return Ok(HandleResult::Skip);
        }

        if intent.selection.is_empty() {
            return Ok(HandleResult::NotApplicable("Select a song to start radio"));
        }

        if !intent.selection.has_only(&[ContentType::Track]) {
            return Ok(HandleResult::NotApplicable("Can only start radio from a song"));
        }

        let Some(seed) = intent.selection.first_song().cloned() else {
            return Ok(HandleResult::NotApplicable("No song selected"));
        };

        log::info!(
            "Starting radio from seed: {}. Note: auto-extend not implemented in v1",
            seed.title()
        );

        ctx.queue_mutator().play(PlayIntent::Radio { seed, mix_type: MixType::SongRadio });

        Ok(HandleResult::Done)
    }

    fn priority(&self) -> i32 {
        0
    }

    fn name(&self) -> &'static str {
        "RadioHandler"
    }
}
