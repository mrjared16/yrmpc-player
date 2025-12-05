//! YouTube Music backend with server-client architecture.
//!
//! Architecture:
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              rmpc serve                      │
//! │  ┌──────────┐ ┌──────────┐ ┌──────────┐    │
//! │  │   api    │ │  stream  │ │  server  │    │
//! │  └────┬─────┘ └────┬─────┘ └────┬─────┘    │
//! │       └────────────┼────────────┘           │
//! │              ┌─────▼─────┐                  │
//! │              │  mpv_ipc  │                  │
//! │              └─────┬─────┘                  │
//! │              ┌─────▼─────┐                  │
//! │              │    MPV    │                  │
//! │              └───────────┘                  │
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
//! - `stream` - Stream URL extraction via yt-dlp
//! - `server` - Manages MPV, queue, handles client commands
//! - `client` - Connects to server, implements MusicBackend
//! - `protocol` - IPC message definitions
//!
//! ## Testing
//!
//! Each component can be tested independently:
//! - `api` - Mock HTTP responses
//! - `stream` - Mock yt-dlp execution
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
//!
//! CLI commands:

pub mod api;
pub mod client;
pub mod config;
pub mod details;
pub mod error;
pub mod protocol;
pub mod server;
pub mod services;
pub mod stream;

pub use client::YouTubeClient;
pub use config::YouTubeConfig;
pub use details::{AlbumDetails, ArtistDetails, PlaylistDetails};
pub use error::{YouTubeError, Result};
pub use server::YouTubeServer;
pub use services::{ApiService, PlaybackService, QueueService};
