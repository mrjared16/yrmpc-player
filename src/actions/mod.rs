//! Action System - Intent → Dispatcher → Handler pattern.
//!
//! ## Overview
//!
//! The action system provides a unified way to handle user actions across
//! panes. Instead of each pane implementing its own action logic, they build an
//! Intent and let the ActionDispatcher dispatch to appropriate handlers.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │ Pane builds Intent { action, selection }                       │
//! │     │                                                          │
//! │     ▼                                                          │
//! │ PaneAction::Execute(Intent)                                    │
//! │     │                                                          │
//! │     ▼                                                          │
//! │ Navigator → ActionDispatcher.dispatch(intent)                  │
//! │     │                                                          │
//! │     ▼                                                          │
//! │ Handlers (priority order):                                     │
//! │     YouTubePlayHandler (wraps default, adds pre-extraction)    │
//! │     PlayHandler                                                │
//! │     QueueHandler                                               │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! // In a pane:
//! fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> PaneAction {
//!     if key == KeyCode::Enter {
//!         let selection = self.get_selection();
//!         return PaneAction::Execute(Intent::play(selection));
//!     }
//!     PaneAction::Handled
//! }
//!
//! // In Navigator:
//! fn handle_pane_action(&mut self, action: PaneAction, ctx: &mut Ctx) {
//!     match action {
//!         PaneAction::Execute(intent) => {
//!             self.dispatcher.dispatch(intent, ctx)?;
//!         }
//!         // ...
//!     }
//! }
//! ```

pub mod dispatcher;
pub mod handler;
pub mod handlers;
pub mod intent;

pub use dispatcher::ActionDispatcher;
pub use handler::{BoxedHandler, HandleResult, Handler};
pub use handlers::{PlayHandler, QueueHandler, SaveHandler, TogglePlaybackHandler};
pub use intent::{Intent, IntentKind, Selection};
