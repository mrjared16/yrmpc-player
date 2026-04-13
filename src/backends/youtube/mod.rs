#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::unwrap_used,
    clippy::map_unwrap_or,
    clippy::single_match_else,
    clippy::collapsible_if,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::ignored_unit_patterns,
    clippy::default_trait_access,
    clippy::trivially_copy_pass_by_ref,
    clippy::cast_possible_wrap,
    clippy::needless_pass_by_value,
    clippy::explicit_iter_loop,
    clippy::iter_without_into_iter,
    clippy::elidable_lifetime_names,
    clippy::enum_glob_use,
    clippy::unnecessary_wraps,
    clippy::if_not_else,
    clippy::print_literal,
    clippy::missing_panics_doc,
    clippy::large_stack_arrays,
    clippy::wildcard_imports,
    clippy::used_underscore_binding,
    clippy::unnecessary_debug_formatting,
    clippy::semicolon_if_nothing_returned,
    clippy::assigning_clones
)]

//! YouTube Music backend with server-client architecture.
//!
//! Architecture:
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              rmpc serve                      │
//! │  ┌──────────┐ ┌──────────────┐ ┌──────────┐ │
//! │  │   api    │ │ url_resolver │ │  server  │ │
//! │  └────┬─────┘ └──────┬───────┘ └────┬─────┘ │
//! │       └──────────────┼──────────────┘       │
//! │              ┌───────▼──────┐               │
//! │              │   mpv_ipc    │               │
//! │              └───────┬──────┘               │
//! │              ┌───────▼──────┐               │
//! │              │     MPV      │               │
//! │              └──────────────┘               │
//! └───────────────────┬─────────────────────────┘
//!                     │ Unix Socket (IPC)
//!                     │
//! ┌───────────────────▼─────────────────────────┐
//! │              rmpc (TUI)                      │
//! │              ┌───────────┐                  │
//! │              │  client   │                  │
//! │              └───────────┘                  │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Components
//!
//! - `api` - YouTube Music API wrapper (search, browse)
//! - `url_resolver` - Resolves video IDs to stream URLs (ytx/yt-dlp with cache)
//! - `server` - Manages MPV, queue, handles client commands
//! - `client` - Connects to server, implements MusicBackend
//! - `protocol` - IPC message definitions
//!
//! ## Testing
//!
//! Each component can be tested independently:
//! - `api` - Mock HTTP responses
//! - `url_resolver` - Mock extractor responses
//! - `server` - Send IPC commands directly
//! - `client` - Mock socket connection
//!
//! ## Usage
//!
//! Start server:
//! ```bash
//! rmpc serve
//! ```
//!
//! Connect with TUI:
//! ```bash
//! rmpc
//! ```

pub mod adapter;
pub mod api;
pub mod audio;
pub mod audio_file_manager;
pub mod client;
pub mod config;
pub mod details;
pub mod error;
pub mod extract;
pub mod extractor;
pub mod media;
pub mod mpv; // MPV IPC module - internal to YouTube backend
pub mod protocol;
pub mod range_set;
pub mod server;
pub mod services;
pub mod streaming_audio_file;
pub mod url_resolver;

pub use audio_file_manager::{AudioFileManager, AudioFileManagerConfig};
pub use client::YouTubeProxy;
pub use config::YouTubeConfig;
pub use details::{AlbumDetails, ArtistDetails, PlaylistDetails};
pub use error::{Result, YouTubeError};
// Re-export MPV types for internal use
pub use mpv::{MpvEvent, MpvIpc};
pub use range_set::RangeSet;
pub use server::YouTubeServer;
pub use services::{ApiService, PlaybackService, QueueService};
pub use streaming_audio_file::ProgressiveAudioFile;
