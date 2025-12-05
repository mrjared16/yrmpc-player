// rmpc library crate
// Mirrors main.rs module structure to enable library builds

#![allow(dead_code)]
#![allow(deprecated)]
#![allow(unused_imports)]
#![allow(unused_variables)]

pub mod config;
pub mod core;
pub mod ctx;
pub mod domain;
pub mod app_state;
pub mod mpd;
pub mod player;
pub mod shared;
pub mod ui;

// Internal re-exports (used by modules via crate::X)
pub(crate) use app_state::AppState;
pub(crate) use shared::events::{AppEvent, WorkRequest};
pub(crate) use shared::macros::{status_warn, try_skip};
pub(crate) use shared::mpd_query::{MpdCommand, MpdQuery, MpdQueryResult};
pub(crate) use shared::tmux;
