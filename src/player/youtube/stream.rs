//! Stream URL extraction for YouTube videos.
//! Separated for independent testing - can mock yt-dlp calls.

use std::{
    num::NonZeroUsize,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use lru::LruCache;
use parking_lot::Mutex;

/// Stream URL extractor with caching
pub struct StreamExtractor {
    cache: Arc<Mutex<LruCache<String, (String, Instant)>>>,
    cache_ttl: Duration,
}

impl StreamExtractor {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(100).unwrap()))),
            cache_ttl: Duration::from_secs(3600), // 1 hour
        }
    }

    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Get stream URL for a video ID, using cache if available
    pub fn get_url(&self, video_id: &str) -> Result<String> {
        // Check cache first
        {
            let mut cache = self.cache.lock();
            if let Some((url, timestamp)) = cache.get(video_id) {
                if timestamp.elapsed() < self.cache_ttl {
                    log::debug!("Stream cache hit for {}", video_id);
                    return Ok(url.clone());
                }
            }
        }

        // Cache miss - extract URL
        log::debug!("Stream cache miss for {}, extracting...", video_id);
        let url = self.extract_url(video_id)?;

        // Store in cache
        self.cache.lock().put(video_id.to_string(), (url.clone(), Instant::now()));

        Ok(url)
    }

    /// Extract URL using yt-dlp (can be mocked for testing)
    fn extract_url(&self, video_id: &str) -> Result<String> {
        Self::extract_url_ytdlp(video_id)
    }

    /// Extract stream URL using yt-dlp CLI
    pub fn extract_url_ytdlp(video_id: &str) -> Result<String> {
        let video_url = format!("https://www.youtube.com/watch?v={}", video_id);

        let output = Command::new("yt-dlp")
            .args(["-f", "bestaudio", "-g", &video_url])
            .output()
            .context("Failed to run yt-dlp. Is it installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("yt-dlp failed: {}", stderr.trim()));
        }

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            return Err(anyhow!("yt-dlp returned empty URL"));
        }

        log::debug!("Extracted URL for {} (len={})", video_id, url.len());
        Ok(url)
    }

    /// Clear the cache (useful when URLs expire)
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// Prefetch URLs for multiple video IDs in background
    pub fn prefetch(&self, video_ids: Vec<String>) {
        let cache = Arc::clone(&self.cache);
        let ttl = self.cache_ttl;

        std::thread::spawn(move || {
            for video_id in video_ids {
                // Skip if already cached
                {
                    let cache = cache.lock();
                    if let Some((_, ts)) = cache.peek(&video_id) {
                        if ts.elapsed() < ttl {
                            continue;
                        }
                    }
                }

                log::debug!("Prefetching URL for {}", video_id);
                match Self::extract_url_ytdlp(&video_id) {
                    Ok(url) => {
                        cache.lock().put(video_id, (url, Instant::now()));
                    }
                    Err(e) => {
                        log::warn!("Failed to prefetch {}: {}", video_id, e);
                    }
                }
            }
        });
    }
}

impl Default for StreamExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_hit() {
        let extractor = StreamExtractor::new();

        // Manually populate cache
        extractor
            .cache
            .lock()
            .put("test123".into(), ("https://example.com/stream".into(), Instant::now()));

        // Should get cached value without calling yt-dlp
        let url = extractor.get_url("test123").unwrap();
        assert_eq!(url, "https://example.com/stream");
    }

    #[test]
    fn test_cache_expiry() {
        let extractor = StreamExtractor::new().with_cache_ttl(Duration::from_millis(1));

        // Populate cache
        extractor
            .cache
            .lock()
            .put("test123".into(), ("https://old.com/stream".into(), Instant::now()));

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(10));

        // Cache should be expired, will try to extract (and fail without yt-dlp)
        let result = extractor.get_url("test123");
        assert!(result.is_err()); // Expected - no actual yt-dlp in test
    }

    #[test]
    fn test_clear_cache() {
        let extractor = StreamExtractor::new();
        extractor
            .cache
            .lock()
            .put("test".into(), ("url".into(), Instant::now()));

        assert_eq!(extractor.cache.lock().len(), 1);
        extractor.clear_cache();
        assert_eq!(extractor.cache.lock().len(), 0);
    }
}
