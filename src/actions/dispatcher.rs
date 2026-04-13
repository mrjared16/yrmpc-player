#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::missing_errors_doc
)]

//! ActionDispatcher dispatches intents to registered handlers.
//!
//! Handlers are called in priority order (highest first). The first
//! handler to return `Done` handles the action.

use anyhow::Result;

use super::{
    handler::{BoxedHandler, HandleResult},
    intent::Intent,
};
use crate::ctx::Ctx;

/// Dispatches intents to registered handlers.
#[derive(Default)]
pub struct ActionDispatcher {
    handlers: Vec<BoxedHandler>,
}

impl ActionDispatcher {
    /// Create a new dispatcher.
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    /// Register a handler (mutable).
    pub fn register(&mut self, handler: BoxedHandler) {
        self.handlers.push(handler);
        // Sort by priority (highest first)
        self.handlers.sort_by_key(|h| -h.priority());
    }

    /// Register a handler (builder pattern).
    pub fn with_handler(mut self, handler: BoxedHandler) -> Self {
        self.register(handler);
        self
    }

    /// Dispatch an intent to registered handlers.
    ///
    /// Handlers are called in priority order. Returns the result of the
    /// first handler that handles the action.
    pub fn dispatch(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        let mut last_reason: Option<&'static str> = None;

        for handler in &self.handlers {
            log::trace!("ActionDispatcher: trying {} for {:?}", handler.name(), intent.action);

            match handler.execute(intent, ctx)? {
                HandleResult::Done => {
                    log::debug!("ActionDispatcher: {} handled {:?}", handler.name(), intent.action);
                    return Ok(HandleResult::Done);
                }
                HandleResult::NotApplicable(reason) => {
                    log::trace!("ActionDispatcher: {} not applicable: {}", handler.name(), reason);
                    last_reason = Some(reason);
                }
                HandleResult::Skip => {
                    log::trace!("ActionDispatcher: {} skipped", handler.name());
                }
            }
        }

        // No handler handled the action
        if let Some(reason) = last_reason {
            Ok(HandleResult::NotApplicable(reason))
        } else {
            Ok(HandleResult::NotApplicable("No handler for this action"))
        }
    }
}

impl std::fmt::Debug for ActionDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionDispatcher").field("handler_count", &self.handlers.len()).finish()
    }
}
