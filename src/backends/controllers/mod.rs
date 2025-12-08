//! Backend Controllers
//!
//! This module provides a clean, organized API for interacting with music backends.
//! Instead of 100+ flat methods on `BackendDispatcher`, functionality is grouped
//! into logical controllers.
//!
//! ## Core Controllers (always available)
//!
//! - [`PlaybackController`] - Play, pause, stop, seek, next, previous
//! - [`QueueController`] - Add, remove, reorder songs in the queue
//! - [`StatusProvider`] - Get current playback status and song info
//! - [`VolumeController`] - Volume control
//! - [`LibraryBrowser`] - Search and browse the music library
//!
//! ## Optional Controllers (capability-gated)
//!
//! These return `Option<T>` based on backend capabilities:
//!
//! - [`SavedPlaylistController`] - Save/load playlists (MPD)
//! - [`StickerController`] - Metadata stickers (MPD)
//! - [`OutputController`] - Audio output control (MPD)
//! - [`DatabaseController`] - Database update/rescan (MPD)
//!
//! ## Usage
//!
//! ```ignore
//! // Core operations - always work
//! dispatcher.playback().play()?;
//! dispatcher.queue().add(song, None)?;
//! dispatcher.volume().set(80)?;
//!
//! // Optional operations - check capability first
//! if let Some(playlists) = dispatcher.saved_playlists() {
//!     playlists.save("favorites")?;
//! }
//! ```

mod playback;
mod queue;
mod status;
mod volume;
mod library;
mod saved_playlists;
mod stickers;
mod outputs;
mod database;

pub use playback::PlaybackController;
pub use queue::QueueController;
pub use status::StatusProvider;
pub use volume::VolumeController;
pub use library::LibraryBrowser;
pub use saved_playlists::SavedPlaylistController;
pub use stickers::StickerController;
pub use outputs::OutputController;
pub use database::DatabaseController;
