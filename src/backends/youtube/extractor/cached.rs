//! Cached extractor decorator - adds LRU + TTL caching with request coalescing.
//!
//! Prevents duplicate extractions when multiple callers request the same ID
//! concurrently. Fast path (extract_one) results take priority over slow path
//! (extract_batch) results.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use parking_lot::Mutex;

use super::Extractor;
use crate::shared::{
    cache::{Cache, CacheConfig as SharedCacheConfig},
    dedup::Dedup,
};

#[derive(Clone)]
struct CacheEntry {
    url: String,
    version: u64,
    immediate: bool,
}

fn should_replace(existing: &CacheEntry, new_version: u64, new_immediate: bool) -> bool {
    if new_immediate && !existing.immediate {
        return true;
    }
    if !new_immediate && existing.immediate {
        return false;
    }
    new_version > existing.version
}

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
    cache: Arc<Mutex<Cache<String, CacheEntry>>>,
    dedup: Dedup<String, Result<String, String>>,
    next_version: Arc<AtomicU64>,
    config: CacheConfig,
}

impl<E: Extractor> CachedExtractor<E> {
    /// Create a new cached extractor with default config.
    pub fn new(inner: E) -> Self {
        Self::with_config(inner, CacheConfig::default())
    }

    /// Create a new cached extractor with custom config.
    pub fn with_config(inner: E, config: CacheConfig) -> Self {
        let shared_config = SharedCacheConfig::new(config.max_entries.max(1)).with_ttl(config.ttl);
        Self {
            inner,
            cache: Arc::new(Mutex::new(Cache::new(shared_config))),
            dedup: Dedup::new(),
            next_version: Arc::new(AtomicU64::new(1)),
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

    fn get_cached(&self, video_id: &str) -> Option<String> {
        let mut cache = self.cache.lock();
        cache.get(video_id).map(|entry| entry.url)
    }

    fn try_cache(&self, video_id: &str, url: String, version: u64, immediate: bool) -> bool {
        let mut cache = self.cache.lock();
        let should_write = match cache.get(video_id) {
            Some(existing) => should_replace(&existing, version, immediate),
            None => true,
        };

        if should_write {
            cache.insert(video_id.to_string(), CacheEntry { url, version, immediate });
        }
        should_write
    }

    #[cfg(test)]
    fn put_cached(&self, video_id: String, url: String) {
        let version = self.next_version.fetch_add(1, Ordering::Relaxed);
        self.try_cache(&video_id, url, version, false);
    }
}

// Implement Clone if inner is Clone
impl<E: Extractor + Clone> Clone for CachedExtractor<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cache: Arc::clone(&self.cache),
            dedup: self.dedup.clone(),
            next_version: Arc::clone(&self.next_version),
            config: self.config.clone(),
        }
    }
}

impl<E: Extractor> Extractor for CachedExtractor<E> {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        let mut results = HashMap::new();
        let version = self.next_version.fetch_add(1, Ordering::Relaxed);

        for id in video_ids {
            if let Some(url) = self.get_cached(id) {
                log::debug!("Cache hit for {}", id);
                results.insert(id.clone(), Ok(url));
            } else {
                let id_owned = id.clone();
                let result = self.dedup.get_or_init_sync(id_owned.clone(), || {
                    if let Some(url) = self.get_cached(&id_owned) {
                        return Ok(url);
                    }
                    self.inner.extract_one(&id_owned).map_err(|e| e.to_string())
                });

                match result {
                    Ok(url) => {
                        self.try_cache(&id_owned, url.clone(), version, false);
                        results.insert(id_owned, Ok(url));
                    }
                    Err(error) => {
                        results.insert(id_owned, Err(anyhow!(error)));
                    }
                }
            }
        }

        results
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        let start = std::time::Instant::now();

        if let Some(url) = self.get_cached(video_id) {
            log::info!("[EXTRACT] cache_hit video_id={} elapsed={:?}", video_id, start.elapsed());
            return Ok(url);
        }

        let result = self.dedup.get_or_init_sync(video_id.to_string(), || {
            let version = self.next_version.fetch_add(1, Ordering::Relaxed);
            log::info!("[EXTRACT] cache_miss video_id={} version={}", video_id, version);

            match self.inner.extract_one(video_id) {
                Ok(url) => {
                    self.try_cache(video_id, url.clone(), version, true);
                    log::info!(
                        "[EXTRACT] complete video_id={} elapsed={:?}",
                        video_id,
                        start.elapsed()
                    );
                    Ok(url)
                }
                Err(e) => {
                    log::warn!(
                        "[EXTRACT] failed video_id={} elapsed={:?} error={}",
                        video_id,
                        start.elapsed(),
                        e
                    );
                    Err(e.to_string())
                }
            }
        });

        match result {
            Ok(url) => Ok(url),
            Err(e) => Err(anyhow!("{}", e)),
        }
    }

    fn clear_cache(&self) {
        self.cache.lock().clear();
        self.inner.clear_cache();
    }

    fn is_cached(&self, video_id: &str) -> bool {
        self.get_cached(video_id).is_some() || self.inner.is_cached(video_id)
    }

    fn invalidate(&self, video_id: &str) {
        self.cache.lock().invalidate(video_id);
        self.dedup.invalidate(video_id);
        self.inner.invalidate(video_id);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    use super::*;

    /// Simple mock extractor for basic tests.
    struct MockExtractor;

    impl Extractor for MockExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids.iter().map(|id| (id.clone(), Ok(format!("url_for_{}", id)))).collect()
        }

        fn name(&self) -> &'static str {
            "mock"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    /// Mock extractor that tracks call counts and returns unique URLs per call.
    /// Used to detect duplicate extractions in concurrency tests.
    struct CountingExtractor {
        /// Number of times extract_one was called.
        extract_one_count: AtomicUsize,
        /// Number of times extract_batch was called.
        extract_batch_count: AtomicUsize,
        /// Delay to simulate network latency.
        delay: Duration,
    }

    impl CountingExtractor {
        fn new(delay: Duration) -> Self {
            Self {
                extract_one_count: AtomicUsize::new(0),
                extract_batch_count: AtomicUsize::new(0),
                delay,
            }
        }

        fn extract_one_count(&self) -> usize {
            self.extract_one_count.load(Ordering::SeqCst)
        }

        #[allow(dead_code)]
        fn extract_batch_count(&self) -> usize {
            self.extract_batch_count.load(Ordering::SeqCst)
        }
    }

    impl Extractor for CountingExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            self.extract_batch_count.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.delay);

            // Return unique URL per call to detect if same ID was extracted twice
            let call_num = self.extract_batch_count.load(Ordering::SeqCst);
            video_ids
                .iter()
                .map(|id| (id.clone(), Ok(format!("batch_{}_url_for_{}", call_num, id))))
                .collect()
        }

        fn extract_one(&self, video_id: &str) -> Result<String> {
            let call_num = self.extract_one_count.fetch_add(1, Ordering::SeqCst) + 1;
            thread::sleep(self.delay);
            // Return unique URL per call to detect duplicate extractions
            Ok(format!("single_{}_url_for_{}", call_num, video_id))
        }

        fn name(&self) -> &'static str {
            "counting"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
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

    // ==================== NEW TESTS FOR REQUEST COALESCING ====================

    #[test]
    fn test_concurrent_extract_one_coalesces_requests() {
        // Given: A slow extractor (100ms delay)
        let extractor = CountingExtractor::new(Duration::from_millis(100));
        let cached = Arc::new(CachedExtractor::new(extractor));

        // When: Two threads request the same ID concurrently
        let cached1 = Arc::clone(&cached);
        let cached2 = Arc::clone(&cached);

        let handle1 = thread::spawn(move || cached1.extract_one("same_id"));
        thread::sleep(Duration::from_millis(10)); // Ensure thread1 starts first
        let handle2 = thread::spawn(move || cached2.extract_one("same_id"));

        let url1 = handle1.join().unwrap().unwrap();
        let url2 = handle2.join().unwrap().unwrap();

        // Then: Both get the same URL (coalesced, not extracted twice)
        assert_eq!(url1, url2, "Both threads should get same URL from coalesced request");

        // And: Extractor was only called once (verified via URL containing "single_1_")
        assert!(
            url1.contains("single_1_"),
            "URL should be from first extraction only, got: {}",
            url1
        );
    }

    #[test]
    fn test_extract_batch_and_extract_one_share_dedup_gate() {
        let extractor = CountingExtractor::new(Duration::from_millis(100));
        let cached = Arc::new(CachedExtractor::new(extractor));

        let batch_cached = Arc::clone(&cached);
        let batch_handle = thread::spawn(move || {
            let ids = vec!["shared_id".to_string()];
            batch_cached.extract_batch(&ids)
        });

        thread::sleep(Duration::from_millis(10));

        let one_cached = Arc::clone(&cached);
        let one_handle = thread::spawn(move || one_cached.extract_one("shared_id"));

        let batch_results = batch_handle.join().unwrap();
        let one_result = one_handle.join().unwrap().unwrap();
        let batch_result = batch_results
            .get("shared_id")
            .expect("shared_id should be present")
            .as_ref()
            .expect("batch should resolve shared_id")
            .clone();

        assert_eq!(batch_result, one_result);
        assert_eq!(cached.inner().extract_one_count(), 1);
        assert_eq!(cached.inner().extract_batch_count(), 0);
    }

    #[test]
    fn test_extract_one_does_not_wait_for_prefetch() {
        // Given: A slow extractor where batch takes much longer than single
        let extractor = CountingExtractor::new(Duration::from_millis(50));
        let cached = Arc::new(CachedExtractor::new(extractor));

        // When: Prefetch starts first (will take ~250ms for 5 items)
        let cached_prefetch = Arc::clone(&cached);
        let prefetch_handle = thread::spawn(move || {
            let ids: Vec<String> = (0..5).map(|i| format!("id_{}", i)).collect();
            cached_prefetch.extract_batch(&ids)
        });

        thread::sleep(Duration::from_millis(10));

        // And: extract_one is called for the same ID while prefetch is running
        let start = std::time::Instant::now();
        let url = cached.extract_one("id_0").unwrap();
        let elapsed = start.elapsed();

        prefetch_handle.join().unwrap();

        // Then: extract_one should complete fast (single extraction ~50ms, not waiting
        // for batch ~250ms)
        assert!(
            elapsed < Duration::from_millis(150),
            "extract_one should not wait for prefetch, took {:?}",
            elapsed
        );

        // And: URL should be from single extraction (single_N), not batch (batch_N)
        assert!(
            url.contains("single_"),
            "URL should be from fast path single extraction, got: {}",
            url
        );
    }

    #[test]
    fn test_prefetch_does_not_overwrite_extract_one_result() {
        // Given: An extractor with controllable delays
        struct DelayedExtractor {
            single_delay: Duration,
            batch_delay: Duration,
            call_count: AtomicUsize,
        }

        impl Extractor for DelayedExtractor {
            fn extract_one(&self, video_id: &str) -> Result<String> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
                thread::sleep(self.single_delay);
                Ok(format!("single_{}_url_for_{}", n, video_id))
            }

            fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
                thread::sleep(self.batch_delay);
                video_ids
                    .iter()
                    .map(|id| (id.clone(), Ok(format!("batch_{}_url_for_{}", n, id))))
                    .collect()
            }

            fn name(&self) -> &'static str {
                "delayed"
            }

            fn clear_cache(&self) {}

            fn is_cached(&self, _video_id: &str) -> bool {
                false
            }

            fn invalidate(&self, _video_id: &str) {}
        }

        let extractor = DelayedExtractor {
            single_delay: Duration::from_millis(20),
            batch_delay: Duration::from_millis(100),
            call_count: AtomicUsize::new(0),
        };
        let cached = Arc::new(CachedExtractor::new(extractor));

        // When: Prefetch starts first (but takes longer)
        let cached_prefetch = Arc::clone(&cached);
        let prefetch_handle =
            thread::spawn(move || cached_prefetch.extract_batch(&["contested_id".to_string()]));

        thread::sleep(Duration::from_millis(5));

        // And: extract_one runs and completes BEFORE prefetch
        let fast_url = cached.extract_one("contested_id").unwrap();
        assert!(fast_url.contains("single_"), "Fast path should use single extraction");

        // Wait for prefetch to complete (it should NOT overwrite)
        prefetch_handle.join().unwrap();

        // Then: Cache should still have the fast path result
        let cached_url = cached.extract_one("contested_id").unwrap();

        // If version priority works: cached_url == fast_url (from cache, same URL)
        // If broken: cached_url might be batch_X or single_3 (cache was overwritten,
        // then re-extracted)
        assert_eq!(
            cached_url, fast_url,
            "Cache should preserve fast path result. Expected: {}, Got: {}",
            fast_url, cached_url
        );
    }
}
