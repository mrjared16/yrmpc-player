//! Cached extractor decorator - adds LRU + TTL caching.
//!
//! Wraps any extractor to provide transparent caching.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use lru::LruCache;
use parking_lot::Mutex;

use super::Extractor;

/// Cache configuration.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum number of cached entries (LRU eviction).
    pub max_entries: usize,
    /// Time-to-live for cached entries.
    pub ttl: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 100,
            ttl: Duration::from_secs(3600), // 1 hour (YouTube URLs expire after ~6 hours)
        }
    }
}

impl CacheConfig {
    /// Create config with custom TTL.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Create config with custom max entries.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }
}

/// Cached extractor decorator.
///
/// Wraps any `Extractor` to add transparent LRU + TTL caching.
/// Cache hits return immediately without calling the inner extractor.
/// For batch extraction, only uncached IDs are passed to the inner extractor.
pub struct CachedExtractor<E: Extractor> {
    inner: E,
    cache: Arc<Mutex<LruCache<String, (String, Instant)>>>,
    config: CacheConfig,
}

impl<E: Extractor> CachedExtractor<E> {
    /// Create a new cached extractor with default config.
    pub fn new(inner: E) -> Self {
        Self::with_config(inner, CacheConfig::default())
    }

    /// Create a new cached extractor with custom config.
    pub fn with_config(inner: E, config: CacheConfig) -> Self {
        let max = NonZeroUsize::new(config.max_entries).unwrap_or(NonZeroUsize::new(100).unwrap());
        Self {
            inner,
            cache: Arc::new(Mutex::new(LruCache::new(max))),
            config,
        }
    }

    /// Clear all cached entries.
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// Get number of cached entries.
    pub fn cache_len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Get the inner extractor.
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// Check if a video ID is cached and not expired.
    fn get_cached(&self, video_id: &str) -> Option<String> {
        let mut cache = self.cache.lock();
        if let Some((url, timestamp)) = cache.get(video_id) {
            if timestamp.elapsed() < self.config.ttl {
                return Some(url.clone());
            }
            // Expired - will be replaced on next put
        }
        None
    }

    /// Store a URL in cache.
    fn put_cached(&self, video_id: String, url: String) {
        self.cache.lock().put(video_id, (url, Instant::now()));
    }
}

// Implement Clone if inner is Clone
impl<E: Extractor + Clone> Clone for CachedExtractor<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cache: Arc::clone(&self.cache),
            config: self.config.clone(),
        }
    }
}

impl<E: Extractor> Extractor for CachedExtractor<E> {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        let mut results = HashMap::new();
        let mut uncached = Vec::new();

        // 1. Partition: cached vs uncached
        for id in video_ids {
            if let Some(url) = self.get_cached(id) {
                log::debug!("Cache hit for {}", id);
                results.insert(id.clone(), Ok(url));
            } else {
                uncached.push(id.clone());
            }
        }

        // 2. Extract only uncached IDs
        if !uncached.is_empty() {
            log::debug!(
                "Cache miss for {} IDs, extracting with {}...",
                uncached.len(),
                self.inner.name()
            );
            let extracted = self.inner.extract_batch(&uncached);

            // 3. Store successful extractions in cache
            for (id, result) in extracted {
                if let Ok(ref url) = result {
                    self.put_cached(id.clone(), url.clone());
                }
                results.insert(id, result);
            }
        }

        results
    }

    fn name(&self) -> &'static str {
        // Delegate to inner - we're just a transparent wrapper
        self.inner.name()
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        // Check cache first
        if let Some(url) = self.get_cached(video_id) {
            log::debug!("Cache hit for {}", video_id);
            return Ok(url);
        }

        // Cache miss - extract
        log::debug!("Cache miss for {}, extracting...", video_id);
        let url = self.inner.extract_one(video_id)?;

        // Store in cache
        self.put_cached(video_id.to_string(), url.clone());

        Ok(url)
    }

    fn clear_cache(&self) {
        self.cache.lock().clear();
        // Also clear inner cache if it has one
        self.inner.clear_cache();
    }
}

// Send + Sync are automatically derived if E: Send + Sync
unsafe impl<E: Extractor> Send for CachedExtractor<E> {}
unsafe impl<E: Extractor> Sync for CachedExtractor<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExtractor;

    impl Extractor for MockExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids
                .iter()
                .map(|id| (id.clone(), Ok(format!("url_for_{}", id))))
                .collect()
        }

        fn name(&self) -> &'static str {
            "mock"
        }
    }

    #[test]
    fn test_cache_hit() {
        let cached = CachedExtractor::new(MockExtractor);

        // First call - cache miss
        let url1 = cached.extract_one("abc123").unwrap();
        assert_eq!(url1, "url_for_abc123");
        assert_eq!(cached.cache_len(), 1);

        // Second call - cache hit (same result, but from cache)
        let url2 = cached.extract_one("abc123").unwrap();
        assert_eq!(url2, "url_for_abc123");
    }

    #[test]
    fn test_batch_partial_cache() {
        let cached = CachedExtractor::new(MockExtractor);

        // Pre-populate cache with one ID
        cached.put_cached("id1".to_string(), "cached_url".to_string());

        // Batch extract - id1 from cache, id2 from extractor
        let results = cached.extract_batch(&["id1".to_string(), "id2".to_string()]);

        assert_eq!(results.get("id1").unwrap().as_ref().unwrap(), "cached_url");
        assert_eq!(results.get("id2").unwrap().as_ref().unwrap(), "url_for_id2");
    }

    #[test]
    fn test_cache_expiry() {
        let config = CacheConfig::default().with_ttl(Duration::from_millis(1));
        let cached = CachedExtractor::with_config(MockExtractor, config);

        // First call - cache miss
        let _ = cached.extract_one("abc123").unwrap();

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(10));

        // Should be expired - will call extractor again
        assert!(cached.get_cached("abc123").is_none());
    }

    #[test]
    fn test_clear_cache() {
        let cached = CachedExtractor::new(MockExtractor);
        let _ = cached.extract_one("abc123").unwrap();
        assert_eq!(cached.cache_len(), 1);

        cached.clear_cache();
        assert_eq!(cached.cache_len(), 0);
    }
}
