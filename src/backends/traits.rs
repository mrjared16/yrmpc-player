//! Music player backend traits
//!
//! ## Terminology
//!
//! - **Queue**: User's playlist of songs to play
//! - **Buffer**: MPV's internal 3-track rolling window (YouTube backend only)
//! - **id**: Queue position identifier (assigned when song added to queue)
//! - **file**: Content identifier (YouTube video ID like "dQw4w9WgXcQ", or file path for MPD)
//!
//! ## When to use `id` vs `file`
//!
//! - Use `id` when navigating within the queue (same song can appear multiple times)
//! - Use `file` when checking if it's the same content (search results, toggle detection)
//!
//! ## Trait Hierarchy
//!
//! ```text
//! QueueOperations (clean interface - use this)
//!     └── enqueue(), dequeue(), reorder(), clear_queue(), play_by_id()
//!
//! MusicBackend extends QueueOperations
//!     └── Core: play(), pause(), stop(), next(), previous(), search(), etc.
//!     └── Optional methods have default no-op implementations
//! ```
//!
//! ## Optional Features
//!
//! Methods like `list_playlists()`, `update()`, `outputs()` have default implementations
//! that return empty results or errors. Use `supports(BackendCapability::X)` to check
//! if a backend actually implements a feature.

use std::collections::HashMap;
use anyhow::{Result, bail};

use crate::{
    domain::{Song, Status, QueuePosition},
    mpd::{SingleOrRange, commands::*, mpd_client::Filter, version::Version},
};

/// Features that backends may or may not support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendCapability {
    /// Persistent playlist storage (save/load playlists)
    SavedPlaylists,
    /// Database update/rescan
    DatabaseManagement,
    /// Sticker metadata
    Stickers,
    /// Audio output selection
    OutputControl,
    /// MPD partitions
    Partitions,
    /// Rich metadata (thumbnails, etc)
    RichMetadata,
}

/// Trait for queue operations (Single Responsibility: only queue management)
///
/// NO DEFAULT IMPLEMENTATIONS - all backends MUST implement these methods.
/// This ensures new backends cannot silently lose metadata.
pub trait QueueOperations: Send + Sync {
    /// Add song with full metadata to queue (canonical method)
    ///
    /// Backends extract what they need from the Song:
    /// - YouTube: full metadata (title, artist, thumbnail)
    /// - MPD: only song.uri (URI)
    fn enqueue(&mut self, song: &Song, position: Option<QueuePosition>) -> Result<()>;

    /// Remove song from queue by ID
    fn dequeue(&mut self, id: u32) -> Result<()>;

    /// Move song position in queue (reorder)
    fn reorder(&mut self, from_id: u32, to_id: u32) -> Result<()>;

    /// Clear entire queue
    fn clear_queue(&mut self) -> Result<()>;

    /// Play song by ID
    fn play_by_id(&mut self, id: u32) -> Result<()>;
}

/// Core music backend trait.
///
/// All backends must implement the required methods (no default).
/// Optional features have default no-op implementations.
pub trait MusicBackend: QueueOperations + Send + Sync {
    // =========================================================================
    // BACKEND IDENTIFICATION (required)
    // =========================================================================

    /// Returns the name of this backend (e.g., "MPD", "YouTube")
    fn backend_name(&self) -> &'static str;

    /// Returns all capabilities this backend supports.
    /// Each backend declares its own capabilities (Single Source of Truth).
    fn capabilities(&self) -> &'static [BackendCapability];

    /// Check if this backend supports a specific capability.
    /// Default implementation uses capabilities() - no need to override.
    fn supports(&self, capability: BackendCapability) -> bool {
        self.capabilities().contains(&capability)
    }

    /// Returns whether this backend supports a specific command (for compatibility checks)
    fn supports_command(&self, _command: &str) -> bool {
        true
    }

    /// Downcast to YouTubeBackend if applicable
    fn as_youtube_backend(&mut self) -> Option<&mut crate::backends::youtube::YouTubeBackend> {
        None
    }

    // =========================================================================
    // PLAYBACK CONTROL (required)
    // =========================================================================

    fn play(&mut self) -> Result<()>;
    fn pause(&mut self, state: bool) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<()>;
    fn previous(&mut self) -> Result<()>;
    fn seek_current(&mut self, position: SeekPosition) -> Result<()>;

    // =========================================================================
    // STATUS QUERIES (required)
    // =========================================================================

    fn get_status(&mut self) -> Result<Status>;
    fn playlist_info(&mut self) -> Result<Vec<Song>>;
    fn current_song(&mut self) -> Result<Option<Song>>;

    // =========================================================================
    // VOLUME CONTROL (required)
    // =========================================================================

    fn volume(&mut self) -> Result<u8>;
    fn set_volume(&mut self, volume: ValueChange) -> Result<()>;

    // =========================================================================
    // LIBRARY BROWSING (required for search)
    // =========================================================================

    fn get_search_suggestions(&mut self, query: String) -> Result<Vec<String>>;

    /// Get library data for a specific category (playlists, albums, artists, songs)
    fn get_library(&mut self, category: super::LibraryCategory) -> Result<Vec<LsInfoEntry>>;

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>>;
    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>>;
    fn find(&mut self, filter: &[Filter], window: Option<(u32, u32)>) -> Result<Vec<Song>>;

    // =========================================================================
    // PLAYBACK OPTIONS (optional - defaults do nothing)
    // =========================================================================

    fn repeat(&mut self, _repeat: bool) -> Result<()> { Ok(()) }
    fn random(&mut self, _random: bool) -> Result<()> { Ok(()) }
    fn single(&mut self, _single: OnOffOneshot) -> Result<()> { Ok(()) }
    fn consume(&mut self, _consume: OnOffOneshot) -> Result<()> { Ok(()) }
    fn crossfade(&mut self, _seconds: u32) -> Result<()> { Ok(()) }
    fn shuffle(&mut self, _range: Option<SingleOrRange>) -> Result<()> { Ok(()) }

    // =========================================================================
    // LIBRARY BROWSING (optional - defaults return empty)
    // =========================================================================

    fn list_all(&mut self, _path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn list_tag(&mut self, _tag: Tag, _filter: Option<&[Filter]>) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn count(&mut self, _filter: &[Filter]) -> Result<(usize, std::time::Duration)> {
        Ok((0, std::time::Duration::ZERO))
    }

    // =========================================================================
    // SAVED PLAYLISTS (optional - MPD implements, others return defaults)
    // =========================================================================

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        Ok(vec![])
    }

    fn playlist_info_name(&mut self, _name: &str) -> Result<Vec<Song>> {
        Ok(vec![])
    }

    fn load_playlist(&mut self, _name: &str, _position: Option<QueuePosition>) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn save_queue_as_playlist(&mut self, _name: &str, _mode: Option<SaveMode>) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn delete_playlist(&mut self, _name: &str) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn rename_playlist(&mut self, _old_name: &str, _new_name: &str) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn add_to_playlist(&mut self, _playlist: &str, _uri: &str) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn delete_from_playlist(&mut self, _playlist: &str, _position: u32) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    fn move_in_playlist(&mut self, _playlist: &str, _from: SingleOrRange, _to: u32) -> Result<()> {
        bail!("Saved playlists not supported by this backend")
    }

    // =========================================================================
    // STICKERS (optional - MPD implements, others return defaults)
    // =========================================================================

    fn list_stickers(&mut self, _uri: &str) -> Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }

    fn set_sticker(&mut self, _uri: &str, _key: &str, _value: &str) -> Result<()> {
        bail!("Stickers not supported by this backend")
    }

    fn delete_sticker(&mut self, _uri: &str, _key: &str) -> Result<()> {
        bail!("Stickers not supported by this backend")
    }

    // =========================================================================
    // DATABASE MANAGEMENT (optional - MPD implements, others return defaults)
    // =========================================================================

    fn update(&mut self, _path: Option<&str>) -> Result<u32> {
        bail!("Database management not supported by this backend")
    }

    fn rescan(&mut self, _path: Option<&str>) -> Result<u32> {
        bail!("Database management not supported by this backend")
    }

    // =========================================================================
    // SYSTEM INFO (optional - return defaults for non-MPD)
    // =========================================================================

    fn version(&self) -> Version {
        Version::new(0, 0, 0)
    }

    fn outputs(&mut self) -> Result<Vec<Output>> {
        Ok(vec![])
    }

    fn enable_output(&mut self, _id: u32) -> Result<()> {
        bail!("Output control not supported by this backend")
    }

    fn disable_output(&mut self, _id: u32) -> Result<()> {
        bail!("Output control not supported by this backend")
    }

    fn toggle_output(&mut self, _id: u32) -> Result<()> {
        bail!("Output control not supported by this backend")
    }

    fn decoders(&mut self) -> Result<Vec<Decoder>> {
        Ok(vec![])
    }

    fn partitions(&mut self) -> Result<Vec<String>> {
        Ok(vec![])
    }

    // =========================================================================
    // DEPRECATED QUEUE METHODS (use QueueOperations instead)
    // =========================================================================

    #[deprecated(note = "Use enqueue(&Song) from QueueOperations trait instead")]
    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()>;

    #[deprecated(note = "Use dequeue(id) from QueueOperations trait instead")]
    fn delete_id(&mut self, id: u32) -> Result<()>;

    #[deprecated(note = "Use clear_queue() from QueueOperations trait instead")]
    fn clear(&mut self) -> Result<()>;

    #[deprecated(note = "Use reorder(from, to) from QueueOperations trait instead")]
    fn move_id(&mut self, from: u32, to: u32) -> Result<()>;

    #[deprecated(note = "Use play_by_id(id) from QueueOperations trait instead")]
    fn play_id(&mut self, id: u32) -> Result<()>;
}
