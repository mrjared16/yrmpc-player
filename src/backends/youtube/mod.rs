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
