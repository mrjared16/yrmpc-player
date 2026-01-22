use std::{
    borrow::Borrow,
    hash::Hash,
    num::NonZeroUsize,
    path::Path,
    time::{Duration, SystemTime},
};

use lru::LruCache;

#[derive(Debug, Clone, Copy)]
pub struct CacheConfig {
    pub max_entries: usize,
    pub ttl: Option<Duration>,
    pub max_weight: Option<u64>,
}

impl CacheConfig {
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self { max_entries, ttl: None, max_weight: None }
    }

    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    #[must_use]
    pub fn with_max_weight(mut self, max_weight: u64) -> Self {
        self.max_weight = Some(max_weight);
        self
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self::new(128)
    }
}

pub trait Weigher<K, V> {
    fn weight(&self, key: &K, value: &V) -> u64;
}

pub trait DiskCacheValue {
    fn disk_path(&self) -> &Path;
}

#[derive(Debug, Clone)]
pub enum CacheLookup<V> {
    Hit(V),
    Miss,
    Expired(V),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UnitWeigher;

impl<K, V> Weigher<K, V> for UnitWeigher {
    fn weight(&self, _key: &K, _value: &V) -> u64 {
        1
    }
}

#[derive(Debug, Clone)]
struct Entry<V> {
    value: V,
    inserted_at: SystemTime,
    weight: u64,
}

#[derive(Debug)]
pub struct Cache<K, V, W = UnitWeigher>
where
    K: Eq + Hash,
{
    entries: LruCache<K, Entry<V>>,
    ttl: Option<Duration>,
    max_entries: usize,
    max_weight: Option<u64>,
    total_weight: u64,
    weigher: W,
}

impl<K, V> Cache<K, V, UnitWeigher>
where
    K: Eq + Hash,
{
    #[must_use]
    pub fn new(config: CacheConfig) -> Self {
        Self::with_weigher(config, UnitWeigher)
    }
}

impl<K, V, W> Cache<K, V, W>
where
    K: Eq + Hash,
    W: Weigher<K, V>,
{
    #[must_use]
    pub fn with_weigher(config: CacheConfig, weigher: W) -> Self {
        let max_entries = config.max_entries.max(1);
        Self {
            entries: LruCache::new(
                NonZeroUsize::new(max_entries).expect("max_entries is at least 1"),
            ),
            ttl: config.ttl,
            max_entries,
            max_weight: config.max_weight,
            total_weight: 0,
            weigher,
        }
    }

    pub fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        match self.get_with_status(key) {
            CacheLookup::Hit(value) => Some(value),
            CacheLookup::Miss | CacheLookup::Expired(_) => None,
        }
    }

    pub fn get_with_status<Q>(&mut self, key: &Q) -> CacheLookup<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        let ttl = self.ttl;
        if let Some(entry) = self.entries.get(key) {
            if !is_expired(ttl, entry.inserted_at) {
                return CacheLookup::Hit(entry.value.clone());
            }
        }

        if let Some(entry) = self.entries.pop(key) {
            self.total_weight = self.total_weight.saturating_sub(entry.weight);
            return CacheLookup::Expired(entry.value);
        }

        CacheLookup::Miss
    }

    pub fn touch<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let ttl = self.ttl;
        if let Some(entry) = self.entries.get(key) {
            return !is_expired(ttl, entry.inserted_at);
        }

        if let Some(entry) = self.entries.pop(key) {
            self.total_weight = self.total_weight.saturating_sub(entry.weight);
        }

        false
    }

    pub fn insert(&mut self, key: K, value: V) {
        let _ = self.insert_with_evicted(key, value);
    }

    pub fn insert_with_evicted(&mut self, key: K, value: V) -> Vec<(K, V)> {
        let weight = self.weigher.weight(&key, &value);
        let entry = Entry { value, inserted_at: SystemTime::now(), weight };

        if let Some(previous) = self.entries.put(key, entry) {
            self.total_weight = self.total_weight.saturating_sub(previous.weight);
        }
        self.total_weight = self.total_weight.saturating_add(weight);

        self.evict_excess()
    }

    pub fn invalidate<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let _ = self.invalidate_with_entry(key);
    }

    pub fn invalidate_with_entry<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.entries.pop(key).map(|entry| {
            self.total_weight = self.total_weight.saturating_sub(entry.weight);
            entry.value
        })
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_weight = 0;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn total_weight(&self) -> u64 {
        self.total_weight
    }

    pub fn evict_excess(&mut self) -> Vec<(K, V)> {
        let mut evicted = Vec::new();

        while self.entries.len() > self.max_entries {
            if let Some((key, entry)) = self.pop_lru_entry() {
                evicted.push((key, entry.value));
            }
        }

        if let Some(max_weight) = self.max_weight {
            while self.total_weight > max_weight {
                if let Some((key, entry)) = self.pop_lru_entry() {
                    evicted.push((key, entry.value));
                } else {
                    break;
                }
            }
        }

        evicted
    }

    fn pop_lru_entry(&mut self) -> Option<(K, Entry<V>)> {
        self.entries.pop_lru().map(|(key, entry)| {
            self.total_weight = self.total_weight.saturating_sub(entry.weight);
            (key, entry)
        })
    }
}

#[derive(Debug)]
pub struct DiskCache<K, V, W = UnitWeigher>
where
    K: Eq + Hash,
    V: DiskCacheValue,
{
    index: Cache<K, V, W>,
}

impl<K, V> DiskCache<K, V, UnitWeigher>
where
    K: Eq + Hash,
    V: DiskCacheValue,
{
    #[must_use]
    pub fn new(config: CacheConfig) -> Self {
        Self::with_weigher(config, UnitWeigher)
    }
}

impl<K, V, W> DiskCache<K, V, W>
where
    K: Eq + Hash,
    V: DiskCacheValue,
    W: Weigher<K, V>,
{
    #[must_use]
    pub fn with_weigher(config: CacheConfig, weigher: W) -> Self {
        Self { index: Cache::with_weigher(config, weigher) }
    }

    pub fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        match self.index.get_with_status(key) {
            CacheLookup::Hit(value) => Some(value),
            CacheLookup::Miss => None,
            CacheLookup::Expired(value) => {
                remove_file_best_effort(value.disk_path());
                None
            }
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        for (_, value) in self.index.insert_with_evicted(key, value) {
            remove_file_best_effort(value.disk_path());
        }
    }

    pub fn invalidate<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if let Some(value) = self.index.invalidate_with_entry(key) {
            remove_file_best_effort(value.disk_path());
        }
    }

    pub fn touch<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.index.touch(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[must_use]
    pub fn total_weight(&self) -> u64 {
        self.index.total_weight()
    }

    pub fn evict_excess(&mut self) {
        for (_, value) in self.index.evict_excess() {
            remove_file_best_effort(value.disk_path());
        }
    }
}

fn remove_file_best_effort(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!("Failed to remove cache file {}: {}", path.display(), error);
        }
    }
}

fn is_expired(ttl: Option<Duration>, inserted_at: SystemTime) -> bool {
    match ttl {
        Some(ttl) => inserted_at.elapsed().map_or(true, |elapsed| elapsed >= ttl),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct StringLenWeigher;

    impl Weigher<String, String> for StringLenWeigher {
        fn weight(&self, _key: &String, value: &String) -> u64 {
            value.len() as u64
        }
    }

    #[test]
    fn get_returns_inserted_value() {
        let mut cache = Cache::new(CacheConfig::new(2));
        cache.insert("a".to_string(), "one".to_string());

        assert_eq!(cache.get("a"), Some("one".to_string()));
    }

    #[test]
    fn get_does_not_return_expired_entry() {
        let mut cache = Cache::new(CacheConfig::new(2).with_ttl(Duration::from_millis(1)));
        cache.insert("a".to_string(), "one".to_string());

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn evicts_lru_by_entry_limit() {
        let mut cache = Cache::new(CacheConfig::new(2));
        cache.insert("a".to_string(), "one".to_string());
        cache.insert("b".to_string(), "two".to_string());
        let _ = cache.get("a");
        cache.insert("c".to_string(), "three".to_string());

        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn evicts_lru_by_weight_limit() {
        let mut cache =
            Cache::with_weigher(CacheConfig::new(5).with_max_weight(5), StringLenWeigher);
        cache.insert("a".to_string(), "1234".to_string());
        cache.insert("b".to_string(), "12".to_string());

        assert!(cache.get("a").is_none());
        assert_eq!(cache.get("b"), Some("12".to_string()));
    }

    #[test]
    fn invalidate_and_clear_remove_entries() {
        let mut cache = Cache::new(CacheConfig::new(3));
        cache.insert("a".to_string(), "one".to_string());
        cache.insert("b".to_string(), "two".to_string());

        cache.invalidate("a");
        assert!(cache.get("a").is_none());
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.get("b").is_none());
    }

    #[derive(Debug, Clone)]
    struct TestDiskValue {
        path: std::path::PathBuf,
        size: u64,
    }

    impl DiskCacheValue for TestDiskValue {
        fn disk_path(&self) -> &Path {
            &self.path
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct TestDiskValueWeigher;

    impl Weigher<String, TestDiskValue> for TestDiskValueWeigher {
        fn weight(&self, _key: &String, value: &TestDiskValue) -> u64 {
            value.size
        }
    }

    #[test]
    fn disk_cache_deletes_files_on_weight_eviction() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut cache = DiskCache::with_weigher(
            CacheConfig::new(10).with_max_weight(150),
            TestDiskValueWeigher,
        );

        let path_a = temp_dir.path().join("a.bin");
        let path_b = temp_dir.path().join("b.bin");
        std::fs::write(&path_a, [1u8; 100]).unwrap();
        std::fs::write(&path_b, [2u8; 100]).unwrap();

        cache.insert("a".to_string(), TestDiskValue { path: path_a.clone(), size: 100 });
        cache.insert("b".to_string(), TestDiskValue { path: path_b.clone(), size: 100 });

        assert!(!path_a.exists());
        assert!(path_b.exists());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.total_weight(), 100);
    }

    #[test]
    fn disk_cache_invalidate_deletes_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut cache = DiskCache::with_weigher(
            CacheConfig::new(10).with_max_weight(500),
            TestDiskValueWeigher,
        );

        let path = temp_dir.path().join("invalidate.bin");
        std::fs::write(&path, [3u8; 80]).unwrap();

        cache.insert("key".to_string(), TestDiskValue { path: path.clone(), size: 80 });
        cache.invalidate("key");

        assert!(!path.exists());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.total_weight(), 0);
    }
}
