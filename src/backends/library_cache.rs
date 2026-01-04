use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use lru::LruCache;

use super::library_category::LibraryCategory;
use crate::mpd::commands::LsInfoEntry;

/// Cache entry with timestamp for TTL checking
#[derive(Debug, Clone)]
struct CacheEntry {
    data: Vec<LsInfoEntry>,
    timestamp: Instant,
}

/// Library data cache with LRU eviction and TTL
#[derive(Debug)]
pub struct LibraryCache {
    /// Per-category caches
    playlists: LruCache<(), CacheEntry>,
    albums: LruCache<(), CacheEntry>,
    artists: LruCache<(), CacheEntry>,
    songs: LruCache<(), CacheEntry>,

    /// Time-to-live for cache entries (default: 24 hours)
    ttl: Duration,

    /// Statistics for debugging
    hits: u64,
    misses: u64,
}

impl LibraryCache {
    /// Create new library cache with default TTL (24 hours)
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(24 * 3600))
    }

    /// Create library cache with custom TTL
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            // Single entry per category (key is unit type)
            playlists: LruCache::new(NonZeroUsize::new(1).unwrap()),
            albums: LruCache::new(NonZeroUsize::new(1).unwrap()),
            artists: LruCache::new(NonZeroUsize::new(1).unwrap()),
            songs: LruCache::new(NonZeroUsize::new(1).unwrap()),
            ttl,
            hits: 0,
            misses: 0,
        }
    }

    /// Get cached data for a category if valid (not expired)
    /// Get cached data for a category if valid (not expired)
    pub fn get(&mut self, category: LibraryCategory) -> Option<Vec<LsInfoEntry>> {
        let ttl = self.ttl; // Copy TTL first to avoid borrow issues

        // Check if entry exists and is not expired
        let (is_expired, has_entry, data) = {
            let cache = self.get_cache_mut(category);
            if let Some(entry) = cache.get(&()) {
                let expired = entry.timestamp.elapsed() >= ttl;
                let data = if !expired { Some(entry.data.clone()) } else { None };
                (expired, true, data)
            } else {
                (false, false, None)
            }
        }; // cache borrow ends here

        if has_entry {
            if let Some(data) = data {
                self.hits += 1;
                log::debug!("Library cache HIT for {:?}", category);
                return Some(data);
            } else if is_expired {
                log::debug!("Library cache EXPIRED for {:?}", category);
                self.get_cache_mut(category).pop(&());
            }
        }

        self.misses += 1;
        log::debug!("Library cache MISS for {:?}", category);
        None
    }

    /// Store data in cache for a category
    pub fn put(&mut self, category: LibraryCategory, data: Vec<LsInfoEntry>) {
        let cache = self.get_cache_mut(category);
        let entry = CacheEntry { data, timestamp: Instant::now() };
        cache.put((), entry);
        log::debug!(
            "Library cache STORED for {:?} ({} items)",
            category,
            cache.peek(&()).map(|e| e.data.len()).unwrap_or(0)
        );
    }

    /// Clear cache for a specific category
    pub fn clear_category(&mut self, category: LibraryCategory) {
        let cache = self.get_cache_mut(category);
        cache.clear();
        log::debug!("Library cache CLEARED for {:?}", category);
    }

    /// Clear all cached library data
    pub fn clear_all(&mut self) {
        self.playlists.clear();
        self.albums.clear();
        self.artists.clear();
        self.songs.clear();
        log::debug!("Library cache CLEARED all categories");
    }

    /// Get cache statistics (hits, misses, hit rate)
    pub fn stats(&self) -> CacheStats {
        let total = self.hits + self.misses;
        let hit_rate = if total > 0 { (self.hits as f64 / total as f64) * 100.0 } else { 0.0 };

        CacheStats { hits: self.hits, misses: self.misses, hit_rate }
    }

    /// Get mutable reference to the appropriate cache
    fn get_cache_mut(&mut self, category: LibraryCategory) -> &mut LruCache<(), CacheEntry> {
        match category {
            LibraryCategory::Playlists => &mut self.playlists,
            LibraryCategory::Albums => &mut self.albums,
            LibraryCategory::Artists => &mut self.artists,
            LibraryCategory::Songs => &mut self.songs,
        }
    }
}

impl Default for LibraryCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Cache statistics for debugging
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}
