//! MPD (Music Player Daemon) backend.
//!
//! This backend connects to an external MPD server and implements
//! the MusicBackend trait by translating commands to the MPD protocol.
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

mod backend;
pub mod protocol;

pub use backend::MpdBackend;
