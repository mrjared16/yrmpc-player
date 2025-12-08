//! MPV IPC module for YouTube backend
//!
//! This module provides low-level MPV communication for PlaybackService.
//! It is internal to the YouTube backend and not intended for standalone use.
//!
//! For YouTube playback, the flow is:
//! - YouTubeBackend -> PlaybackService -> MpvIpc -> MPV process

pub mod ipc;

pub use ipc::{MpvIpc, MpvEvent};
