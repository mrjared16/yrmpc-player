// rmpc library crate
// Mirrors main.rs module structure to enable library builds

#![allow(dead_code)]
#![allow(deprecated)]
#![allow(unused_imports)]
#![allow(unused_variables)]

pub mod app_state;
pub mod backends;
pub mod config;
pub mod core;
pub mod ctx;
pub mod domain;
pub mod player;
pub mod shared;
pub mod ui;

#[cfg(test)]
pub mod tests;

// Backward compatibility: re-export mpd protocol at crate::mpd
// This allows existing `use crate::mpd::*` imports to continue working
pub use backends::mpd::protocol as mpd;

// Internal re-exports (used by modules via crate::X)
pub(crate) use app_state::AppState;
pub(crate) use shared::events::{AppEvent, WorkRequest};
pub(crate) use shared::macros::{status_warn, try_skip};
pub(crate) use backends::messaging::{PlayerCommand, Query, QueryResult};
pub(crate) use shared::tmux;
