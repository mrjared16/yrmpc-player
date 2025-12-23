//! Backend implementations for streaming music playback.
//!
//! # Architecture Overview
//!
//! ```text
//! TUI ──► BackendDispatcher ──► YouTubeProxy ──IPC──► Daemon ──► MPV
//!                           └──► MpdBackend ──TCP──► MPD Server
//! ```
//!
//! # Two-Layer API Design
//!
//! This module uses a **two-layer abstraction** for backend operations:
//!
//! ## Layer 1: `api::` traits (Backend-Agnostic)
//!
//! Minimal, universal traits that both backends implement:
//! - [`api::Playback`] - play, pause, stop, seek, status
//! - [`api::Queue`] - add, remove, clear, move items  
//! - [`api::Discovery`] - search, browse, suggestions
//! - [`api::Volume`] - get/set volume
//!
//! **Use these when**: Writing code that works identically on MPD and YouTube.
//!
//! ```ignore
//! dispatcher.playback().play()?;
//! dispatcher.queue().add(&items, InsertAt::End, AfterAdd::None)?;
//! ```
//!
//! ## Layer 2: `MusicBackend` trait (Rich, MPD-Flavored)
//!
//! Full-featured trait with all MPD capabilities:
//! - Stickers, saved playlists, outputs, database management
//! - Rich `domain::Status` with all MPD fields
//! - YouTube implements stubs for unsupported features
//!
//! **Use these when**: You need MPD-specific features or rich status data.
//!
//! ```ignore
//! // For rich status (UI needs songid, playlist version, etc.)
//! let status = dispatcher.status().get()?;  // Returns domain::Status
//!
//! // For MPD-specific features
//! if let Some(stickers) = dispatcher.stickers() {
//!     stickers.set(uri, "rating", "5")?;
//! }
//! ```
//!
//! ## Status Types
//!
//! There are two status types by design:
//! - [`api::Status`] - Minimal (state, position, volume, repeat, shuffle)
//! - [`domain::Status`] - Rich (all fields: songid, playlist, consume, etc.)
//!
//! The [`PlaybackController`] uses `api::Status` for toggle logic.
//! The [`StatusProvider`] uses `domain::Status` for UI display.
//!
//! # For LLM Agents
//!
//! - **YouTube bugs**: `api/` + `youtube/` directory. IGNORE `mpd/`.
//! - **MPD bugs**: `api/` + `mpd/` directory. IGNORE `youtube/`.
//!
//! # Module Guide
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`api`] | Backend-agnostic traits: `Playback`, `Queue`, `Discovery` |
//! | [`client`] | `BackendDispatcher` routes to active backend |
//! | [`controllers`] | Typed controller wrappers for clean API |
//! | [`traits`] | `MusicBackend` trait (rich, MPD-flavored) |
//! | [`mpd`] | MPD backend implementation |
//! | [`youtube`] | YouTube backend (daemon + MPV player) |

pub mod api;
pub mod client;
pub mod controllers;
pub mod interaction;
pub mod library_cache;
pub mod library_category;
pub mod messaging;
pub mod mpd;
pub mod traits;
pub mod youtube;

// === API types (backend-agnostic) ===
pub use api::{
    ContentType, Item, SearchQuery, SearchResults, BrowseResult,
    State, Repeat, Status as ApiStatus, Capability,
    InsertAt, AfterAdd,
    Playback, Queue, Discovery, Volume, Backend,
};

// === Legacy types (MPD-flavored, for migration) ===
pub use client::BackendDispatcher;
pub use library_category::LibraryCategory;
pub use mpd::MpdBackend;
pub use traits::{MusicBackend, QueueOperations, BackendCapability};
pub use youtube::YouTubeProxy;

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
