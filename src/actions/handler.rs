#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

//! Handler trait and HandleResult for the Action System.
//!
//! Handlers process specific action types. They are registered with the
//! ActionDispatcher and called in priority order.

use anyhow::Result;

use super::intent::Intent;
use crate::ctx::Ctx;

/// Result of executing a handler.
#[derive(Debug)]
pub enum HandleResult {
    /// Action was handled successfully.
    Done,
    /// Cannot handle this action (let next handler try).
    NotApplicable(&'static str),
    /// Skip to next handler without error message.
    Skip,
}

/// A handler processes specific action types.
///
/// Handlers are called in priority order. The first handler that returns
/// `HandleResult::Done` wins. If all handlers return `NotApplicable` or `Skip`,
/// the last error message is shown to the user.
pub trait Handler: Send + Sync {
    /// Execute the action.
    ///
    /// Returns:
    /// - `Done` if handled successfully
    /// - `NotApplicable(reason)` if this handler can't handle the action
    /// - `Skip` to silently pass to the next handler
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult>;

    /// Priority for ordering handlers. Higher = runs first.
    /// Default is 0.
    fn priority(&self) -> i32 {
        0
    }

    /// Human-readable name for debugging.
    fn name(&self) -> &'static str {
        "Handler"
    }
}

/// A boxed handler for dynamic dispatch.
pub type BoxedHandler = Box<dyn Handler>;
