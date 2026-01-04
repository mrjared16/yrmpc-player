//! MPV IPC module for YouTube backend
//!
//! This module provides low-level MPV communication for PlaybackService.
//! It is internal to the YouTube backend and not intended for standalone use.
//!
//! For YouTube playback, the flow is:
//! - YouTubeProxy -> PlaybackService -> MpvIpc -> MPV process

pub mod ipc;

pub use ipc::{MpvEvent, MpvIpc};
