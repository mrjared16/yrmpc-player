#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::unwrap_used,
    clippy::double_ended_iterator_last,
    clippy::redundant_closure,
    clippy::enum_glob_use,
    clippy::elidable_lifetime_names,
    clippy::useless_conversion,
    clippy::unnecessary_cast,
    clippy::missing_panics_doc,
    clippy::iter_without_into_iter
)]

//! MPD (Music Player Daemon) backend.
//!
//! This backend connects to an external MPD server and implements
//! the api traits by translating commands to the MPD protocol.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐     MPD Protocol      ┌─────────────┐
//! │ MpdBackend  │ ──────────────────────▶│ MPD Server  │
//! └─────────────┘     (TCP/Unix)        └─────────────┘
//! ```
//!
//! The `protocol` submodule contains the MPD protocol implementation.
//!
//! ## Trait Implementation
//!
//! ### Universal (api::*)
//! - `api::Playback` - play, pause, stop, seek, crossfade
//! - `api::Queue` - add, remove, clear, single, consume
//! - `api::Discovery` - search, browse
//! - `api::Volume` - get, set
//!
//! ### MPD-Specific (specific::*)
//! - [`specific::Stickers`] - Metadata tags
//! - [`specific::Outputs`] - Audio output control
//! - [`specific::Database`] - Library update/rescan
//!
//! ## Deprecated
//!
//! The `optional` module is deprecated. Use:
//! - `specific` for MPD-only features
//! - `api::optional::Playlists` for playlist operations
//! - `api::Queue` for single/consume modes
//! - `api::Playback` for crossfade

mod api_impl;
mod backend;
pub mod protocol;
pub mod specific;
mod specific_impl;

// Legacy modules - deprecated, will be removed
#[deprecated(since = "0.1.0", note = "Use specific module instead")]
pub mod optional;
#[allow(deprecated)]
mod optional_impl;

pub use backend::MpdBackend;
// Re-export deprecated traits for backward compatibility
#[allow(deprecated)]
pub use optional::{Modes, SavedPlaylists};
pub use specific::{Database, Outputs, Stickers};
