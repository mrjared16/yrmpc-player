//! Queue Store - Single source of truth for queue state with optimistic
//! updates.
//!
//! This module implements the Observer pattern for queue management:
//! - Optimistic updates for instant UI feedback
//! - Automatic UI notification on changes
//! - Daemon reconciliation when backend responds
//!
//! # Architecture
//!
//! ```text
//! User Action -> Queue.add() -> 1. Optimistic local update
//!                            -> 2. Notify UI (QueueEvent::Changed)
//!                            -> 3. Send command to daemon
//!                            -> 4. Request reconciliation
//!
//! Daemon Response -> Queue.reconcile() -> Replace local with daemon snapshot
//!                                      -> Notify UI
//! ```

use std::{
    collections::HashSet,
    ops::Deref,
    sync::{
        Arc, RwLock, RwLockReadGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use crossbeam::channel::Sender;

use crate::{
    AppEvent,
    backends::youtube::protocol::play_intent::{PlayIntent, RequestId},
    domain::Song,
};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> RequestId {
    REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Wrapper for RwLockReadGuard that derefs through Arc to Vec<Song>
pub struct QueueReadGuard<'a> {
    _guard: RwLockReadGuard<'a, Arc<Vec<Song>>>,
    data: Arc<Vec<Song>>,
}

impl Deref for QueueReadGuard<'_> {
    type Target = Vec<Song>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

/// Trait for daemon communication - allows mocking in tests
pub trait QueueDaemon: Send + Sync + 'static {
    /// Add songs to the daemon's queue
    fn add(&self, songs: Vec<Song>);
    /// Add songs and play the first one
    fn add_and_play(&self, songs: Vec<Song>);
    /// Remove songs by their IDs
    fn remove_ids(&self, ids: Vec<u32>);
    /// Move a song to a new position
    fn move_id(&self, id: u32, to_position: u32);
    /// Clear the entire queue
    fn clear(&self);
    /// Request a queue refresh from daemon (for reconciliation)
    fn refresh(&self);
    /// Play with explicit intent (new PlayIntent-based API)
    fn play_with_intent(&self, intent: PlayIntent, request_id: RequestId);
}

pub struct QueueStore {
    inner: Arc<RwLock<Arc<Vec<Song>>>>,
    ui_tx: Sender<AppEvent>,
    daemon: Arc<dyn QueueDaemon>,
}

impl std::fmt::Debug for QueueStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueStore").field("len", &self.inner.read().map(|q| q.len())).finish()
    }
}

impl QueueStore {
    pub(crate) fn new(
        initial: Vec<Song>,
        ui_tx: Sender<AppEvent>,
        daemon: Arc<dyn QueueDaemon>,
    ) -> Self {
        Self { inner: Arc::new(RwLock::new(Arc::new(initial))), ui_tx, daemon }
    }

    // ========== READ API ==========

    /// Cheap snapshot - returns Arc clone, not Vec clone (pointer-width copy)
    pub fn snapshot(&self) -> Arc<Vec<Song>> {
        Arc::clone(&self.inner.read().expect("queue lock poisoned"))
    }

    /// Get a read guard to the queue (for iteration without cloning)
    pub fn read(&self) -> QueueReadGuard<'_> {
        let guard = self.inner.read().expect("queue lock poisoned");
        let data = Arc::clone(&guard);
        QueueReadGuard { _guard: guard, data }
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.inner.read().expect("queue lock poisoned").len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.inner.read().expect("queue lock poisoned").is_empty()
    }

    /// Get a song by index (cloned to avoid lock issues)
    pub fn get(&self, idx: usize) -> Option<Song> {
        self.inner.read().expect("queue lock poisoned").get(idx).cloned()
    }

    /// Find current song in queue by ID
    pub fn find_by_id(&self, song_id: u32) -> Option<(usize, Song)> {
        self.inner
            .read()
            .expect("queue lock poisoned")
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == Some(song_id))
            .map(|(idx, s)| (idx, s.clone()))
    }

    // ========== WRITE API (Optimistic + Notify + Command) ==========

    pub fn add(&self, songs: Vec<Song>) {
        if songs.is_empty() {
            return;
        }
        log::info!("QueueStore: adding {} song(s)", songs.len());

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            let mut new_queue = (**guard).clone();
            let optimistic: Vec<Song> = songs
                .iter()
                .cloned()
                .map(|mut s| {
                    s.id = None;
                    s
                })
                .collect();
            new_queue.extend(optimistic);
            *guard = Arc::new(new_queue);
        }

        self.notify();
        self.daemon.add(songs);
        self.daemon.refresh();
    }

    pub fn add_and_play(&self, songs: Vec<Song>) {
        if songs.is_empty() {
            return;
        }
        log::info!("QueueStore: adding {} song(s) and playing first", songs.len());

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            let mut new_queue = (**guard).clone();
            let optimistic: Vec<Song> = songs
                .iter()
                .cloned()
                .map(|mut s| {
                    s.id = None;
                    s
                })
                .collect();
            new_queue.extend(optimistic);
            *guard = Arc::new(new_queue);
        }

        self.notify();
        self.daemon.add_and_play(songs);
        self.daemon.refresh();
    }

    pub fn remove_ids(&self, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        log::info!("QueueStore: removing {} song(s)", ids.len());

        let id_set: HashSet<u32> = ids.iter().copied().collect();

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            let new_queue: Vec<Song> = (**guard)
                .iter()
                .filter(|s| s.id.map(|id| !id_set.contains(&id)).unwrap_or(true))
                .cloned()
                .collect();
            *guard = Arc::new(new_queue);
        }

        self.notify();
        self.daemon.remove_ids(ids.to_vec());
        self.daemon.refresh();
    }

    pub fn move_id(&self, id: u32, to_index: usize) {
        log::info!("QueueStore: moving song {} to position {}", id, to_index);

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            let mut new_queue = (**guard).clone();
            let Some(from) = new_queue.iter().position(|s| s.id == Some(id)) else {
                return;
            };

            let song = new_queue.remove(from);
            let to = to_index.min(new_queue.len());
            new_queue.insert(to, song);
            *guard = Arc::new(new_queue);
        }

        self.notify();
        self.daemon.move_id(id, to_index as u32);
        self.daemon.refresh();
    }

    pub fn clear(&self) {
        log::info!("QueueStore: clearing queue");

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            *guard = Arc::new(Vec::new());
        }

        self.notify();
        self.daemon.clear();
        self.daemon.refresh();
    }

    /// Deprecated: Use `play(PlayIntent::Context { ... })` instead.
    /// This method will be removed in a future version.
    #[deprecated(since = "0.1.0", note = "Use play(PlayIntent::Context) instead")]
    pub fn replace_and_play(&self, songs: Vec<Song>) {
        if songs.is_empty() {
            self.clear();
            return;
        }
        log::info!("QueueStore: replacing queue with {} song(s) and playing first", songs.len());

        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            let optimistic: Vec<Song> = songs
                .iter()
                .cloned()
                .map(|mut s| {
                    s.id = None;
                    s
                })
                .collect();
            *guard = Arc::new(optimistic);
        }

        self.notify();
        self.daemon.clear();
        self.daemon.add_and_play(songs);
        self.daemon.refresh();
    }

    pub fn reconcile(&self, backend_queue: Vec<Song>) {
        log::debug!("QueueStore: reconciling with {} songs from daemon", backend_queue.len());
        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            *guard = Arc::new(backend_queue);
        }
        self.notify();
    }

    /// Play with explicit intent (new PlayIntent-based API)
    pub fn play(&self, intent: PlayIntent) {
        let request_id = next_request_id();

        // Optimistic local update - adapt to available QueueStore methods
        match &intent {
            PlayIntent::Context { tracks, offset: _, shuffle: _, source: _ } => {
                // For Context, replace the entire queue
                let optimistic: Vec<Song> = tracks
                    .iter()
                    .cloned()
                    .map(|mut s| {
                        s.id = None;
                        s
                    })
                    .collect();
                {
                    let mut guard = self.inner.write().expect("queue lock poisoned");
                    *guard = Arc::new(optimistic);
                }
            }
            PlayIntent::Next { tracks } => {
                // For Next, append to queue (no insert_after_current available)
                {
                    let mut guard = self.inner.write().expect("queue lock poisoned");
                    let mut new_queue = (**guard).clone();
                    let optimistic: Vec<Song> = tracks
                        .iter()
                        .cloned()
                        .map(|mut s| {
                            s.id = None;
                            s
                        })
                        .collect();
                    new_queue.extend(optimistic);
                    *guard = Arc::new(new_queue);
                }
            }
            PlayIntent::Append { tracks } => {
                // For Append, add to end
                {
                    let mut guard = self.inner.write().expect("queue lock poisoned");
                    let mut new_queue = (**guard).clone();
                    let optimistic: Vec<Song> = tracks
                        .iter()
                        .cloned()
                        .map(|mut s| {
                            s.id = None;
                            s
                        })
                        .collect();
                    new_queue.extend(optimistic);
                    *guard = Arc::new(new_queue);
                }
            }
            PlayIntent::Radio { seed, mix_type: _ } => {
                // For Radio, replace with seed track
                let mut optimistic = seed.clone();
                optimistic.id = None;
                {
                    let mut guard = self.inner.write().expect("queue lock poisoned");
                    *guard = Arc::new(vec![optimistic]);
                }
            }
        }

        // Notify UI
        self.notify();

        // Send to daemon (fire-and-forget)
        self.daemon.play_with_intent(intent, request_id);
    }

    // ========== INTERNAL ==========

    fn notify(&self) {
        let _ = self.ui_tx.send(AppEvent::RequestRender);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct MockDaemon {
        add_count: AtomicUsize,
        remove_count: AtomicUsize,
        clear_count: AtomicUsize,
        refresh_count: AtomicUsize,
    }

    impl MockDaemon {
        fn new() -> Self {
            Self {
                add_count: AtomicUsize::new(0),
                remove_count: AtomicUsize::new(0),
                clear_count: AtomicUsize::new(0),
                refresh_count: AtomicUsize::new(0),
            }
        }
    }

    impl QueueDaemon for MockDaemon {
        fn add(&self, _songs: Vec<Song>) {
            self.add_count.fetch_add(1, Ordering::SeqCst);
        }

        fn add_and_play(&self, _songs: Vec<Song>) {
            self.add_count.fetch_add(1, Ordering::SeqCst);
        }

        fn remove_ids(&self, _ids: Vec<u32>) {
            self.remove_count.fetch_add(1, Ordering::SeqCst);
        }

        fn move_id(&self, _id: u32, _to: u32) {}

        fn clear(&self) {
            self.clear_count.fetch_add(1, Ordering::SeqCst);
        }

        fn refresh(&self) {
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
        }

        fn play_with_intent(&self, _intent: PlayIntent, _request_id: RequestId) {
            // No-op for tests
        }
    }

    fn create_test_song(name: &str) -> Song {
        use std::collections::HashMap;
        Song {
            uri: format!("{}.mp3", name),
            metadata: HashMap::from([("title".to_string(), vec![name.to_string()])]),
            ..Default::default()
        }
    }

    #[test]
    fn test_add_optimistic() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueStore::new(vec![], tx, daemon.clone());

        store.add(vec![create_test_song("song1")]);

        assert_eq!(store.len(), 1);
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(daemon.refresh_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_reconcile_replaces_state() {
        let (tx, _rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueStore::new(vec![create_test_song("old")], tx, daemon);

        store.reconcile(vec![create_test_song("new1"), create_test_song("new2")]);

        assert_eq!(store.len(), 2);
        let queue = store.read();
        assert!(queue[0].uri.contains("new1"));
        assert!(queue[1].uri.contains("new2"));
    }

    #[test]
    fn test_clear_optimistic() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueStore::new(
            vec![create_test_song("song1"), create_test_song("song2")],
            tx,
            daemon.clone(),
        );

        store.clear();

        assert!(store.is_empty());
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.clear_count.load(Ordering::SeqCst), 1);
    }
}
