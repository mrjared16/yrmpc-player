//! Audio data cache for instant playback.
//!
//! Downloads first N seconds of audio via HTTP Range requests and caches
//! to disk. This eliminates the ~0.5-1s network latency when starting playback.
//!
//! # Architecture
//!
//! ```text
//! URL Resolved → AudioCache.prefetch(video_id, url)
//!                    │
//!                    ▼
//!              HTTP GET with Range: bytes=0-{calculated}
//!                    │
//!                    ▼
//!              Save to ~/.cache/rmpc/audio/{video_id}.opus
//!                    │
//!                    ▼
//! On Play → AudioCache.get_path(video_id) → Some(path) → Build EDL URL
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use parking_lot::RwLock;

/// Configuration for audio cache.
#[derive(Debug, Clone)]
pub struct AudioCacheConfig {
    /// Directory to store cached audio files.
    pub cache_dir: PathBuf,
    /// Duration of audio to prefetch (in seconds).
    pub prefetch_duration_secs: f64,
    /// Maximum cache size in bytes (0 = unlimited).
    pub max_size_bytes: u64,
    /// Maximum age of cached files before cleanup.
    pub max_age: Duration,
    /// Default bitrate for byte calculation (bits per second).
    pub default_bitrate_bps: u32,
}

impl Default for AudioCacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rmpc")
                .join("audio"),
            prefetch_duration_secs: 10.0,
            max_size_bytes: 100 * 1024 * 1024, // 100 MB
            max_age: Duration::from_secs(24 * 60 * 60), // 24 hours
            default_bitrate_bps: 160_000, // 160 kbps (Opus default)
        }
    }
}

/// Entry in the audio cache.
#[derive(Debug, Clone)]
struct CacheEntry {
    path: PathBuf,
    size_bytes: u64,
    cached_duration_secs: f64,
    created_at: SystemTime,
}

/// Audio data cache for instant playback.
///
/// Prefetches the first N seconds of audio for queue songs,
/// enabling instant playback without network latency.
pub struct AudioCache {
    config: AudioCacheConfig,
    /// Map of video_id → cache entry.
    entries: RwLock<HashMap<String, CacheEntry>>,
    /// HTTP client for fetching audio.
    client: reqwest::blocking::Client,
}

impl AudioCache {
    /// Create a new audio cache with default configuration.
    pub fn new() -> Result<Self> {
        Self::with_config(AudioCacheConfig::default())
    }

    /// Create a new audio cache with custom configuration.
    pub fn with_config(config: AudioCacheConfig) -> Result<Self> {
        // Ensure cache directory exists
        fs::create_dir_all(&config.cache_dir)
            .with_context(|| format!("Failed to create cache dir: {:?}", config.cache_dir))?;

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let cache = Self {
            config,
            entries: RwLock::new(HashMap::new()),
            client,
        };

        // Load existing cache entries
        cache.scan_cache_dir();

        Ok(cache)
    }

    /// Scan cache directory and populate entries map.
    fn scan_cache_dir(&self) {
        let mut entries = self.entries.write();

        if let Ok(dir_entries) = fs::read_dir(&self.config.cache_dir) {
            for entry in dir_entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "opus" || ext == "webm") {
                    if let Some(video_id) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(metadata) = fs::metadata(&path) {
                            entries.insert(video_id.to_string(), CacheEntry {
                                path: path.clone(),
                                size_bytes: metadata.len(),
                                cached_duration_secs: self.config.prefetch_duration_secs,
                                created_at: metadata.created().unwrap_or(SystemTime::now()),
                            });
                        }
                    }
                }
            }
        }

        log::debug!("Loaded {} entries from audio cache", entries.len());
    }

    /// Calculate bytes needed for a given duration at the specified bitrate.
    fn calculate_bytes(&self, duration_secs: f64, bitrate_bps: u32) -> u64 {
        let bytes_per_second = (bitrate_bps as f64) / 8.0;
        let bytes = bytes_per_second * duration_secs;
        // Add 20% margin for container overhead
        (bytes * 1.2) as u64
    }

    /// Get the cache file path for a video ID.
    fn cache_path(&self, video_id: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{}.opus", video_id))
    }

    /// Check if a video is cached.
    pub fn has(&self, video_id: &str) -> bool {
        self.entries.read().contains_key(video_id)
    }

    /// Get the cache path for a video if it exists.
    pub fn get_path(&self, video_id: &str) -> Option<PathBuf> {
        self.entries.read().get(video_id).map(|e| e.path.clone())
    }

    /// Get the cached duration for a video.
    pub fn get_cached_duration(&self, video_id: &str) -> Option<f64> {
        self.entries.read().get(video_id).map(|e| e.cached_duration_secs)
    }

    /// Prefetch audio for a video ID.
    ///
    /// Downloads the first N seconds of audio and caches to disk.
    /// This is a blocking operation - call from a background thread.
    pub fn prefetch(&self, video_id: &str, stream_url: &str) -> Result<PathBuf> {
        // Check if already cached
        if let Some(path) = self.get_path(video_id) {
            log::debug!("Audio already cached: {}", video_id);
            return Ok(path);
        }

        // Calculate bytes to download
        let bytes_needed = self.calculate_bytes(
            self.config.prefetch_duration_secs,
            self.config.default_bitrate_bps,
        );

        log::debug!(
            "Prefetching {} bytes (~{}s) for {}",
            bytes_needed,
            self.config.prefetch_duration_secs,
            video_id
        );

        // Download with Range header
        let response = self.client
            .get(stream_url)
            .header("Range", format!("bytes=0-{}", bytes_needed - 1))
            .send()
            .with_context(|| format!("Failed to fetch audio for {}", video_id))?;

        // Check response status
        if !response.status().is_success() && response.status().as_u16() != 206 {
            anyhow::bail!(
                "HTTP error {} fetching audio for {}",
                response.status(),
                video_id
            );
        }

        // Get actual content length
        let content_length = response.content_length().unwrap_or(bytes_needed);

        // Read response body
        let bytes = response.bytes()
            .with_context(|| format!("Failed to read audio bytes for {}", video_id))?;

        // Save to cache file
        let cache_path = self.cache_path(video_id);
        let mut file = File::create(&cache_path)
            .with_context(|| format!("Failed to create cache file: {:?}", cache_path))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write cache file: {:?}", cache_path))?;

        // Add to entries map
        {
            let mut entries = self.entries.write();
            entries.insert(video_id.to_string(), CacheEntry {
                path: cache_path.clone(),
                size_bytes: bytes.len() as u64,
                cached_duration_secs: self.config.prefetch_duration_secs,
                created_at: SystemTime::now(),
            });
        }

        log::info!(
            "Cached {} bytes of audio for {} at {:?}",
            bytes.len(),
            video_id,
            cache_path
        );

        // Check if we need to evict old entries
        self.maybe_evict();

        Ok(cache_path)
    }

    /// Prefetch audio in background thread.
    pub fn prefetch_async(&self, video_id: String, stream_url: String) {
        // Clone what we need for the thread
        let cache_dir = self.config.cache_dir.clone();
        let prefetch_duration = self.config.prefetch_duration_secs;
        let bitrate = self.config.default_bitrate_bps;
        let entries = Arc::clone(&Arc::new(self.entries.read().clone()));

        std::thread::spawn(move || {
            // Quick check if already cached
            if entries.contains_key(&video_id) {
                return;
            }

            // Create a simple client for this thread
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("Failed to create HTTP client for prefetch: {}", e);
                    return;
                }
            };

            let bytes_needed = ((bitrate as f64 / 8.0) * prefetch_duration * 1.2) as u64;

            match client
                .get(&stream_url)
                .header("Range", format!("bytes=0-{}", bytes_needed - 1))
                .send()
            {
                Ok(response) => {
                    if response.status().is_success() || response.status().as_u16() == 206 {
                        if let Ok(bytes) = response.bytes() {
                            let cache_path = cache_dir.join(format!("{}.opus", video_id));
                            if let Ok(mut file) = File::create(&cache_path) {
                                if file.write_all(&bytes).is_ok() {
                                    log::info!("Background cached {} bytes for {}", bytes.len(), video_id);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to prefetch audio for {}: {}", video_id, e);
                }
            }
        });
    }

    /// Evict old entries if cache is too large.
    fn maybe_evict(&self) {
        let mut entries = self.entries.write();

        // Calculate total size
        let total_size: u64 = entries.values().map(|e| e.size_bytes).sum();

        if total_size <= self.config.max_size_bytes {
            return;
        }

        // Sort by age (oldest first)
        let mut sorted: Vec<_> = entries.iter().collect();
        sorted.sort_by_key(|(_, e)| e.created_at);

        // Evict oldest until under limit
        let mut current_size = total_size;
        let mut to_remove = Vec::new();

        for (video_id, entry) in sorted {
            if current_size <= self.config.max_size_bytes {
                break;
            }

            // Remove file
            if let Err(e) = fs::remove_file(&entry.path) {
                log::warn!("Failed to remove cache file {:?}: {}", entry.path, e);
            } else {
                current_size -= entry.size_bytes;
                to_remove.push(video_id.clone());
                log::debug!("Evicted cached audio: {}", video_id);
            }
        }

        // Remove from map
        for video_id in to_remove {
            entries.remove(&video_id);
        }
    }

    /// Clean up old cache entries.
    pub fn cleanup_old(&self) {
        let now = SystemTime::now();
        let mut entries = self.entries.write();
        let mut to_remove = Vec::new();

        for (video_id, entry) in entries.iter() {
            if let Ok(age) = now.duration_since(entry.created_at) {
                if age > self.config.max_age {
                    if let Err(e) = fs::remove_file(&entry.path) {
                        log::warn!("Failed to remove old cache file {:?}: {}", entry.path, e);
                    } else {
                        to_remove.push(video_id.clone());
                        log::debug!("Removed old cached audio: {} (age: {:?})", video_id, age);
                    }
                }
            }
        }

        for video_id in to_remove {
            entries.remove(&video_id);
        }
    }

    /// Get cache statistics.
    pub fn stats(&self) -> AudioCacheStats {
        let entries = self.entries.read();
        let total_size: u64 = entries.values().map(|e| e.size_bytes).sum();

        AudioCacheStats {
            entry_count: entries.len(),
            total_size_bytes: total_size,
            cache_dir: self.config.cache_dir.clone(),
        }
    }
}

impl Default for AudioCache {
    fn default() -> Self {
        Self::new().expect("Failed to create default audio cache")
    }
}

/// Audio cache statistics.
#[derive(Debug, Clone)]
pub struct AudioCacheStats {
    pub entry_count: usize,
    pub total_size_bytes: u64,
    pub cache_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_calculation() {
        let config = AudioCacheConfig::default();
        let cache = AudioCache::with_config(config).unwrap();

        // 10s at 160kbps = 200KB * 1.2 = 240KB
        let bytes = cache.calculate_bytes(10.0, 160_000);
        assert!(bytes >= 200_000 && bytes <= 250_000);
    }
}
