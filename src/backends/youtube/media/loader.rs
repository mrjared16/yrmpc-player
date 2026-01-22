//! Audio loading/caching trait

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;

/// Audio loading and caching interface
///
/// Abstracts audio download and cache management for gapless playback.
/// Based on patterns from `AudioCache::ensure_prefix()` and related methods.
#[async_trait]
pub trait AudioLoader: Send + Sync {
    /// Ensure audio prefix is cached for gapless playback
    ///
    /// Downloads the first `prefix_bytes` of the track if not already cached.
    /// Returns the path to the cached file.
    ///
    /// # Arguments
    /// * `track_id` - Unique identifier for the track (e.g., YouTube video ID)
    /// * `url` - Stream URL to download from
    /// * `prefix_bytes` - Number of bytes to cache (typically 128KB-256KB)
    ///
    /// # Returns
    /// Path to the cached file on success
    async fn ensure_prefix(&self, track_id: &str, url: &str, prefix_bytes: u64) -> Result<PathBuf>;

    /// Check if audio prefix is already cached
    ///
    /// # Arguments
    /// * `track_id` - Unique identifier for the track
    ///
    /// # Returns
    /// `true` if the prefix exists in cache, `false` otherwise
    fn is_cached(&self, track_id: &str) -> bool;

    /// Get the path to a cached file if it exists
    ///
    /// This is a convenience method that returns `Some(path)` if the audio
    /// is cached, `None` otherwise. Implementations may simply check
    /// `is_cached()` and return the expected path.
    ///
    /// # Arguments
    /// * `track_id` - Unique identifier for the track
    ///
    /// # Returns
    /// `Some(PathBuf)` with the cached file path, or `None` if not cached
    fn cached_path(&self, track_id: &str) -> Option<PathBuf>;
}
