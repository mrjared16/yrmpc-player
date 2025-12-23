//! Backend implementations for music playback.
//!
//! # Architecture Overview
//!
//! This module contains all backend implementations for music playback:
//!
//! - **`mpd`**: MPD (Music Player Daemon) client backend - connects to external MPD server
//! - **`youtube`**: YouTube Music backend with internal MPV player and daemon architecture
//!
//! ## Key Components
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`traits`] | Core `MusicBackend` and `QueueOperations` traits |
//! | [`client`] | `BackendDispatcher` enum that unifies all backends |
//! | [`interaction`] | High-level `BackendActions` trait for TUI interaction |
//! | [`messaging`] | Request/response types for async backend communication |
//!
//! ## For New Developers
//!
//! **Start here:**
//! 1. [`BackendDispatcher`] - The unified entry point for all backend operations (will be renamed to `BackendDispatcher`)
//! 2. [`BackendActions`] - High-level actions used by TUI components
//! 3. [`MusicBackend`] - The core trait that all backends implement
//!
//! ## Communication Flow
//!
//! ```text
//! TUI Component
//!     │
//!     ▼
//! BackendDispatcher::resolve_and_enqueue()  (via BackendActions trait)
//!     │
//!     ▼
//! ClientRequest sent to backend thread (via messaging)
//!     │
//!     ▼
//! BackendDispatcher dispatches to MPD or YouTube
//!     │
//!     ▼
//! QueryResult returned to TUI
//! ```

pub mod client;
pub mod controllers;
pub mod interaction;
pub mod library_cache;
pub mod library_category;
pub mod messaging;
pub mod mpd;
pub mod traits;
pub mod youtube;

// Re-export commonly used types
pub use client::BackendDispatcher;
pub use library_category::LibraryCategory;
pub use mpd::MpdBackend;
pub use traits::{MusicBackend, QueueOperations, BackendCapability};
pub use youtube::{YouTubeBackend, YouTubeProxy};

// Re-export controllers for the new organized API
pub use controllers::{
    PlaybackController, QueueController, StatusProvider, VolumeController,
    LibraryBrowser, SavedPlaylistController, StickerController, 
    OutputController, DatabaseController,
};

// Re-export MPV types from YouTube backend for backward compatibility
// MPV is now internal to the YouTube backend
pub use youtube::mpv::{MpvIpc, MpvEvent};

// Re-export messaging types
pub use messaging::{
    ClientRequest, PlayerCommand, Query, QueryResult, QuerySync, PreviewGroup,
    EXTERNAL_COMMAND, GLOBAL_STATUS_UPDATE, GLOBAL_VOLUME_UPDATE,
    GLOBAL_QUEUE_UPDATE, GLOBAL_STICKERS_UPDATE, run_status_update,
};

// Re-export interaction types - using new name BackendActions
pub use interaction::{
    BackendActions, Enqueue, DeleteTarget, PartitionedOutput, PartitionedOutputKind,
};
