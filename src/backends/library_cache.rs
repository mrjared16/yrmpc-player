use std::time::Duration;

use super::library_category::LibraryCategory;
use crate::{
    mpd::commands::LsInfoEntry,
    shared::cache::{Cache, CacheConfig},
};

/// Library data cache with LRU eviction and TTL
#[derive(Debug)]
pub struct LibraryCache {
    /// Per-category caches
    playlists: Cache<(), Vec<LsInfoEntry>>,
    albums: Cache<(), Vec<LsInfoEntry>>,
    artists: Cache<(), Vec<LsInfoEntry>>,
    songs: Cache<(), Vec<LsInfoEntry>>,

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
        let config = CacheConfig::new(1).with_ttl(ttl);
        Self {
            // Single entry per category (key is unit type)
            playlists: Cache::new(config),
            albums: Cache::new(config),
            artists: Cache::new(config),
            songs: Cache::new(config),
            hits: 0,
            misses: 0,
        }
    }

    /// Get cached data for a category if valid (not expired)
    pub fn get(&mut self, category: LibraryCategory) -> Option<Vec<LsInfoEntry>> {
        if let Some(data) = self.get_cache_mut(category).get(&()) {
            self.hits += 1;
            log::debug!("Library cache HIT for {:?}", category);
            return Some(data);
        }

        self.misses += 1;
        log::debug!("Library cache MISS for {:?}", category);
        None
    }

    /// Store data in cache for a category
    pub fn put(&mut self, category: LibraryCategory, data: Vec<LsInfoEntry>) {
        let cache = self.get_cache_mut(category);
        let count = data.len();
        cache.insert((), data);
        log::debug!("Library cache STORED for {:?} ({} items)", category, count);
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
    fn get_cache_mut(&mut self, category: LibraryCategory) -> &mut Cache<(), Vec<LsInfoEntry>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_cache_hit_and_miss_stats() {
        let mut cache = LibraryCache::with_ttl(Duration::from_secs(60));

        assert!(cache.get(LibraryCategory::Albums).is_none());
        cache.put(LibraryCategory::Albums, Vec::new());
        assert_eq!(cache.get(LibraryCategory::Albums), Some(Vec::new()));

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn ttl_expiration_is_enforced() {
        let mut cache = LibraryCache::with_ttl(Duration::from_millis(1));
        cache.put(LibraryCategory::Playlists, Vec::new());

        std::thread::sleep(Duration::from_millis(10));

        assert!(cache.get(LibraryCategory::Playlists).is_none());
    }
}
