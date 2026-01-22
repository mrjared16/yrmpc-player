use std::{
    future::Future,
    hash::Hash,
    sync::{
        Arc,
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use dashmap::{DashMap, mapref::entry::Entry};
use tokio::sync::OnceCell;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DedupError {
    #[error("Computation timed out after {0:?}")]
    Timeout(Duration),
}

/// Request deduplication abstraction.
///
/// Multiple concurrent requests for the same key share a single computation.
/// First caller becomes "leader" and runs the closure; others wait for result.
///
/// Automatically removes slots after computation to prevent memory leaks.
#[derive(Clone)]
pub struct Dedup<K, V>
where
    K: Eq + Hash,
{
    slots: Arc<DashMap<K, Arc<AsyncSlot<V>>>>,
    sync_slots: Arc<DashMap<K, Arc<SyncSlot<V>>>>,
}

#[derive(Debug)]
struct AsyncSlot<V> {
    cell: OnceCell<V>,
    users: AtomicUsize,
}

impl<V> AsyncSlot<V> {
    fn new() -> Self {
        Self { cell: OnceCell::new(), users: AtomicUsize::new(1) }
    }
}

#[derive(Debug)]
struct SyncSlot<V> {
    cell: OnceLock<V>,
    users: AtomicUsize,
}

impl<V> SyncSlot<V> {
    fn new() -> Self {
        Self { cell: OnceLock::new(), users: AtomicUsize::new(1) }
    }
}

impl<K: std::fmt::Debug + Eq + Hash, V> std::fmt::Debug for Dedup<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dedup")
            .field("slots", &self.slots.len())
            .field("sync_slots", &self.sync_slots.len())
            .finish()
    }
}

impl<K, V> Dedup<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn new() -> Self {
        Self { slots: Arc::new(DashMap::new()), sync_slots: Arc::new(DashMap::new()) }
    }

    fn acquire_async_slot(&self, key: K) -> Arc<AsyncSlot<V>> {
        match self.slots.entry(key) {
            Entry::Occupied(entry) => {
                let slot = entry.get().clone();
                slot.users.fetch_add(1, Ordering::AcqRel);
                slot
            }
            Entry::Vacant(entry) => {
                let slot = Arc::new(AsyncSlot::new());
                entry.insert(slot.clone());
                slot
            }
        }
    }

    fn acquire_sync_slot(&self, key: K) -> Arc<SyncSlot<V>> {
        match self.sync_slots.entry(key) {
            Entry::Occupied(entry) => {
                let slot = entry.get().clone();
                slot.users.fetch_add(1, Ordering::AcqRel);
                slot
            }
            Entry::Vacant(entry) => {
                let slot = Arc::new(SyncSlot::new());
                entry.insert(slot.clone());
                slot
            }
        }
    }

    fn release_async_slot(&self, key: &K, slot: &Arc<AsyncSlot<V>>) {
        if slot.users.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }

        if let Entry::Occupied(entry) = self.slots.entry(key.clone()) {
            if Arc::ptr_eq(entry.get(), slot) {
                entry.remove();
            }
        }
    }

    fn release_sync_slot(&self, key: &K, slot: &Arc<SyncSlot<V>>) {
        if slot.users.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }

        if let Entry::Occupied(entry) = self.sync_slots.entry(key.clone()) {
            if Arc::ptr_eq(entry.get(), slot) {
                entry.remove();
            }
        }
    }

    /// Async get-or-compute with automatic cleanup.
    ///
    /// Only ONE caller runs the compute closure; others await the result.
    /// Slot is removed after all waiters receive their clone.
    pub async fn get_or_init<F, Fut>(&self, key: K, compute: F) -> V
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = V>,
    {
        struct Guard<
            'a,
            K: Eq + Hash + Clone + Send + Sync + 'static,
            V: Clone + Send + Sync + 'static,
        > {
            dedup: &'a Dedup<K, V>,
            key: K,
            slot: Arc<AsyncSlot<V>>,
        }
        impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Clone + Send + Sync + 'static> Drop
            for Guard<'_, K, V>
        {
            fn drop(&mut self) {
                self.dedup.release_async_slot(&self.key, &self.slot);
            }
        }

        let slot = self.acquire_async_slot(key.clone());
        let _guard = Guard { dedup: self, key, slot: slot.clone() };

        slot.cell.get_or_init(compute).await.clone()
    }

    /// Async get-or-compute with timeout.
    ///
    /// Returns [`DedupError::Timeout`] if the computation takes longer than
    /// `timeout`.
    pub async fn get_or_init_with_timeout<F, Fut>(
        &self,
        key: K,
        timeout: Duration,
        compute: F,
    ) -> Result<V, DedupError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = V>,
    {
        struct Guard<
            'a,
            K: Eq + Hash + Clone + Send + Sync + 'static,
            V: Clone + Send + Sync + 'static,
        > {
            dedup: &'a Dedup<K, V>,
            key: K,
            slot: Arc<AsyncSlot<V>>,
        }
        impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Clone + Send + Sync + 'static> Drop
            for Guard<'_, K, V>
        {
            fn drop(&mut self) {
                self.dedup.release_async_slot(&self.key, &self.slot);
            }
        }

        let slot = self.acquire_async_slot(key.clone());
        let _guard = Guard { dedup: self, key, slot: slot.clone() };

        match tokio::time::timeout(timeout, slot.cell.get_or_init(compute)).await {
            Ok(v) => Ok(v.clone()),
            Err(_) => Err(DedupError::Timeout(timeout)),
        }
    }

    /// Sync version for blocking contexts.
    ///
    /// Only ONE caller runs the compute closure; others block waiting for
    /// result. Slot is removed after computation completes.
    pub fn get_or_init_sync<F>(&self, key: K, compute: F) -> V
    where
        F: FnOnce() -> V,
    {
        struct Guard<
            'a,
            K: Eq + Hash + Clone + Send + Sync + 'static,
            V: Clone + Send + Sync + 'static,
        > {
            dedup: &'a Dedup<K, V>,
            key: K,
            slot: Arc<SyncSlot<V>>,
        }
        impl<K: Eq + Hash + Clone + Send + Sync + 'static, V: Clone + Send + Sync + 'static> Drop
            for Guard<'_, K, V>
        {
            fn drop(&mut self) {
                self.dedup.release_sync_slot(&self.key, &self.slot);
            }
        }

        let slot = self.acquire_sync_slot(key.clone());
        let _guard = Guard { dedup: self, key, slot: slot.clone() };

        slot.cell.get_or_init(compute).clone()
    }

    /// Explicitly remove an entry (for TTL or error recovery).
    pub fn invalidate<Q>(&self, key: &Q)
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.slots.remove(key);
        self.sync_slots.remove(key);
    }

    /// Check if a computation is currently in progress for this key.
    pub fn is_pending<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.slots.contains_key(key) || self.sync_slots.contains_key(key)
    }
}

impl<K, V> Default for Dedup<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;

    #[tokio::test]
    async fn test_single_computation() {
        let dedup: Dedup<String, i32> = Dedup::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone = counter.clone();
        let result = dedup
            .get_or_init("key".to_string(), || async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                42
            })
            .await;

        assert_eq!(result, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_concurrent_dedup() {
        let dedup: Dedup<String, i32> = Dedup::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let dedup = dedup.clone();
                let counter = counter.clone();
                tokio::spawn(async move {
                    dedup
                        .get_or_init("key".to_string(), || async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            42
                        })
                        .await
                })
            })
            .collect();

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        assert!(results.iter().all(|&r| r == 42));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_slot_removed_after_completion() {
        let dedup: Dedup<String, i32> = Dedup::new();

        let _ = dedup.get_or_init("key".to_string(), || async { 42 }).await;

        assert!(!dedup.is_pending(&"key".to_string()));
    }

    #[tokio::test]
    async fn test_different_keys_run_independently() {
        let dedup: Dedup<String, i32> = Dedup::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter1 = counter.clone();
        let counter2 = counter.clone();

        let (r1, r2) = tokio::join!(
            dedup.get_or_init("key1".to_string(), || async move {
                counter1.fetch_add(1, Ordering::SeqCst);
                1
            }),
            dedup.get_or_init("key2".to_string(), || async move {
                counter2.fetch_add(1, Ordering::SeqCst);
                2
            }),
        );

        assert_eq!(r1, 1);
        assert_eq!(r2, 2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_result_type_dedup() {
        let dedup: Dedup<String, Result<i32, String>> = Dedup::new();

        let result = dedup.get_or_init("key".to_string(), || async { Ok(42) }).await;

        assert_eq!(result, Ok(42));
    }

    #[test]
    fn test_sync_computation() {
        let dedup: Dedup<String, i32> = Dedup::new();

        let result = dedup.get_or_init_sync("key".to_string(), || 42);

        assert_eq!(result, 42);
    }

    #[test]
    fn test_sync_deduplication() {
        use std::{thread, time::Duration};

        let dedup: Dedup<String, i32> = Dedup::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let dedup1 = dedup.clone();
        let counter1 = counter.clone();
        let handle1 = thread::spawn(move || {
            dedup1.get_or_init_sync("key".to_string(), || {
                counter1.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(50));
                42
            })
        });

        thread::sleep(Duration::from_millis(10));

        let dedup2 = dedup.clone();
        let handle2 = thread::spawn(move || {
            dedup2.get_or_init_sync("key".to_string(), || {
                // This should not run
                100
            })
        });

        let r1 = handle1.join().unwrap();
        let r2 = handle2.join().unwrap();

        assert_eq!(r1, 42);
        assert_eq!(r2, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_timeout_cleanup() {
        let dedup: Dedup<String, i32> = Dedup::new();

        let result = dedup
            .get_or_init_with_timeout("key".to_string(), Duration::from_millis(10), || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                42
            })
            .await;

        assert_eq!(result, Err(DedupError::Timeout(Duration::from_millis(10))));
        assert!(!dedup.is_pending(&"key".to_string()));
    }

    #[tokio::test]
    async fn test_panic_cleanup() {
        let dedup: Dedup<String, i32> = Dedup::new();

        let dedup_clone = dedup.clone();
        let handle = tokio::spawn(async move {
            dedup_clone
                .get_or_init_with_timeout("key".to_string(), Duration::from_secs(1), || async {
                    panic!("intentional panic");
                })
                .await
        });

        let _ = handle.await;

        assert!(!dedup.is_pending(&"key".to_string()));
    }

    #[tokio::test]
    async fn test_concurrent_timeout_cleanup() {
        let dedup: Dedup<String, i32> = Dedup::new();

        let h1 = {
            let dedup = dedup.clone();
            tokio::spawn(async move {
                dedup
                    .get_or_init_with_timeout(
                        "key".to_string(),
                        Duration::from_millis(10),
                        || async {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            42
                        },
                    )
                    .await
            })
        };

        let h2 = {
            let dedup = dedup.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                dedup
                    .get_or_init_with_timeout(
                        "key".to_string(),
                        Duration::from_millis(10),
                        || async { 100 },
                    )
                    .await
            })
        };

        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();

        assert_eq!(r1, Err(DedupError::Timeout(Duration::from_millis(10))));
        assert_eq!(r2, Ok(100));
    }

    #[tokio::test]
    async fn timeout_allows_follower_retry_with_fresh_compute() {
        let dedup: Dedup<String, i32> = Dedup::new();
        let leader_counter = Arc::new(AtomicUsize::new(0));
        let follower_counter = Arc::new(AtomicUsize::new(0));

        let leader = {
            let dedup = dedup.clone();
            let counter = leader_counter.clone();
            tokio::spawn(async move {
                dedup
                    .get_or_init_with_timeout(
                        "shared".to_string(),
                        Duration::from_millis(10),
                        || async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            42
                        },
                    )
                    .await
            })
        };

        let follower = {
            let dedup = dedup.clone();
            let counter = follower_counter.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                dedup
                    .get_or_init_with_timeout(
                        "shared".to_string(),
                        Duration::from_millis(150),
                        || async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            99
                        },
                    )
                    .await
            })
        };

        let leader_result = leader.await.unwrap();
        let follower_result = follower.await.unwrap();

        assert_eq!(leader_result, Err(DedupError::Timeout(Duration::from_millis(10))));
        assert_eq!(follower_result, Ok(99));
        assert_eq!(leader_counter.load(Ordering::SeqCst), 1);
        assert_eq!(follower_counter.load(Ordering::SeqCst), 1);
    }
}
