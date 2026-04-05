//! Queue State - single source of truth for queue state with optimistic
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
//! User Action -> QueueMutator.add() -> 1. Optimistic local update
//!                            -> 2. Notify UI (QueueEvent::Changed)
//!                            -> 3. Send command to daemon
//!                            -> 4. Request reconciliation
//!
//! Daemon Response -> QueueState.reconcile_from_backend() -> Replace local with
//! authoritative queue snapshot -> Notify UI
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

pub struct QueueState {
    inner: Arc<RwLock<Arc<Vec<Song>>>>,
    ui_tx: Sender<AppEvent>,
    daemon: Arc<dyn QueueDaemon>,
}

enum QueueMutation {
    Add { songs: Vec<Song> },
    AddAndPlay { songs: Vec<Song> },
    RemoveIds { ids: Vec<u32> },
    MoveId { id: u32, to_index: usize },
    Clear,
    ReplaceAndPlay { songs: Vec<Song> },
    Play { intent: PlayIntent, request_id: RequestId },
}

impl std::fmt::Debug for QueueState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueState").field("len", &self.inner.read().map(|q| q.len())).finish()
    }
}

impl QueueState {
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
        self.submit(QueueMutation::Add { songs });
    }

    pub fn add_and_play(&self, songs: Vec<Song>) {
        self.submit(QueueMutation::AddAndPlay { songs });
    }

    pub fn remove_ids(&self, ids: &[u32]) {
        self.submit(QueueMutation::RemoveIds { ids: ids.to_vec() });
    }

    pub fn move_id(&self, id: u32, to_index: usize) {
        self.submit(QueueMutation::MoveId { id, to_index });
    }

    pub fn clear(&self) {
        self.submit(QueueMutation::Clear);
    }

    /// Deprecated: Use `play(PlayIntent::Context { ... })` instead.
    /// This method will be removed in a future version.
    #[deprecated(since = "0.1.0", note = "Use play(PlayIntent::Context) instead")]
    pub fn replace_and_play(&self, songs: Vec<Song>) {
        self.submit(QueueMutation::ReplaceAndPlay { songs });
    }

    pub(crate) fn reconcile_from_backend(&self, backend_queue: Vec<Song>) {
        log::debug!("QueueState: reconciling with {} songs from daemon", backend_queue.len());
        {
            let mut guard = self.inner.write().expect("queue lock poisoned");
            *guard = Arc::new(backend_queue);
        }
        self.notify();
    }

    /// Play with explicit intent (new PlayIntent-based API)
    pub fn play(&self, intent: PlayIntent) {
        self.submit(QueueMutation::Play { intent, request_id: next_request_id() });
    }

    // ========== INTERNAL ==========

    fn submit(&self, mutation: QueueMutation) {
        if !self.apply_optimistic(&mutation) {
            return;
        }

        self.notify();
        self.dispatch(mutation);
        self.daemon.refresh();
    }

    fn apply_optimistic(&self, mutation: &QueueMutation) -> bool {
        match mutation {
            QueueMutation::Add { songs } => {
                if songs.is_empty() {
                    return false;
                }
                log::info!("QueueState: adding {} song(s)", songs.len());
                self.replace_queue(|queue| {
                    let mut new_queue = queue.clone();
                    new_queue.extend(Self::optimistic_songs(songs));
                    new_queue
                });
                true
            }
            QueueMutation::AddAndPlay { songs } => {
                if songs.is_empty() {
                    return false;
                }
                log::info!("QueueState: adding {} song(s) and playing first", songs.len());
                self.replace_queue(|queue| {
                    let mut new_queue = queue.clone();
                    new_queue.extend(Self::optimistic_songs(songs));
                    new_queue
                });
                true
            }
            QueueMutation::RemoveIds { ids } => {
                if ids.is_empty() {
                    return false;
                }
                log::info!("QueueState: removing {} song(s)", ids.len());
                let id_set: HashSet<u32> = ids.iter().copied().collect();
                self.replace_queue(|queue| {
                    queue
                        .iter()
                        .filter(|s| s.id.map(|id| !id_set.contains(&id)).unwrap_or(true))
                        .cloned()
                        .collect()
                });
                true
            }
            QueueMutation::MoveId { id, to_index } => {
                log::info!("QueueState: moving song {} to position {}", id, to_index);
                let mut moved = false;
                self.replace_queue(|queue| {
                    let mut new_queue = queue.clone();
                    let Some(from) = new_queue.iter().position(|s| s.id == Some(*id)) else {
                        return new_queue;
                    };

                    let song = new_queue.remove(from);
                    let to = (*to_index).min(new_queue.len());
                    new_queue.insert(to, song);
                    moved = true;
                    new_queue
                });
                moved
            }
            QueueMutation::Clear => {
                log::info!("QueueState: clearing queue");
                self.replace_queue(|_| Vec::new());
                true
            }
            QueueMutation::ReplaceAndPlay { songs } => {
                if songs.is_empty() {
                    log::info!(
                        "QueueState: replacing queue with 0 song(s); clearing queue instead"
                    );
                    self.replace_queue(|_| Vec::new());
                } else {
                    log::info!(
                        "QueueState: replacing queue with {} song(s) and playing first",
                        songs.len()
                    );
                    let optimistic = Self::optimistic_songs(songs);
                    self.replace_queue(|_| optimistic.clone());
                }
                true
            }
            QueueMutation::Play { intent, .. } => {
                match intent {
                    PlayIntent::Context { tracks, .. } => {
                        let optimistic = Self::optimistic_songs(tracks);
                        self.replace_queue(|_| optimistic.clone());
                    }
                    PlayIntent::Next { tracks } | PlayIntent::Append { tracks } => {
                        self.replace_queue(|queue| {
                            let mut new_queue = queue.clone();
                            new_queue.extend(Self::optimistic_songs(tracks));
                            new_queue
                        });
                    }
                    PlayIntent::Radio { seed, .. } => {
                        let mut optimistic = seed.clone();
                        optimistic.id = None;
                        self.replace_queue(|_| vec![optimistic.clone()]);
                    }
                }
                true
            }
        }
    }

    fn dispatch(&self, mutation: QueueMutation) {
        match mutation {
            QueueMutation::Add { songs } => self.daemon.add(songs),
            QueueMutation::AddAndPlay { songs } => self.daemon.add_and_play(songs),
            QueueMutation::RemoveIds { ids } => self.daemon.remove_ids(ids),
            QueueMutation::MoveId { id, to_index } => self.daemon.move_id(id, to_index as u32),
            QueueMutation::Clear => self.daemon.clear(),
            QueueMutation::ReplaceAndPlay { songs } => {
                if songs.is_empty() {
                    self.daemon.clear();
                } else {
                    self.daemon.clear();
                    self.daemon.add_and_play(songs);
                }
            }
            QueueMutation::Play { intent, request_id } => {
                self.daemon.play_with_intent(intent, request_id);
            }
        }
    }

    fn replace_queue(&self, update: impl FnOnce(&Vec<Song>) -> Vec<Song>) {
        let mut guard = self.inner.write().expect("queue lock poisoned");
        *guard = Arc::new(update(&guard));
    }

    fn optimistic_songs(songs: &[Song]) -> Vec<Song> {
        songs
            .iter()
            .cloned()
            .map(|mut song| {
                song.id = None;
                song
            })
            .collect()
    }

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
        move_count: AtomicUsize,
        clear_count: AtomicUsize,
        refresh_count: AtomicUsize,
        play_with_intent_count: AtomicUsize,
    }

    impl MockDaemon {
        fn new() -> Self {
            Self {
                add_count: AtomicUsize::new(0),
                remove_count: AtomicUsize::new(0),
                move_count: AtomicUsize::new(0),
                clear_count: AtomicUsize::new(0),
                refresh_count: AtomicUsize::new(0),
                play_with_intent_count: AtomicUsize::new(0),
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

        fn move_id(&self, _id: u32, _to: u32) {
            self.move_count.fetch_add(1, Ordering::SeqCst);
        }

        fn clear(&self) {
            self.clear_count.fetch_add(1, Ordering::SeqCst);
        }

        fn refresh(&self) {
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
        }

        fn play_with_intent(&self, _intent: PlayIntent, _request_id: RequestId) {
            self.play_with_intent_count.fetch_add(1, Ordering::SeqCst);
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
        let store = QueueState::new(vec![], tx, daemon.clone());

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
        let store = QueueState::new(vec![create_test_song("old")], tx, daemon);

        store.reconcile_from_backend(vec![create_test_song("new1"), create_test_song("new2")]);

        assert_eq!(store.len(), 2);
        let queue = store.read();
        assert!(queue[0].uri.contains("new1"));
        assert!(queue[1].uri.contains("new2"));
    }

    #[test]
    fn test_clear_optimistic() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueState::new(
            vec![create_test_song("song1"), create_test_song("song2")],
            tx,
            daemon.clone(),
        );

        store.clear();

        assert!(store.is_empty());
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.clear_count.load(Ordering::SeqCst), 1);
        assert_eq!(daemon.refresh_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_remove_ids_filters_queue_and_refreshes() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueState::new(
            vec![
                Song { id: Some(1), ..create_test_song("song1") },
                Song { id: Some(2), ..create_test_song("song2") },
            ],
            tx,
            daemon.clone(),
        );

        store.remove_ids(&[1]);

        assert_eq!(store.len(), 1);
        assert_eq!(store.get(0).and_then(|song| song.id), Some(2));
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(daemon.refresh_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_move_id_updates_queue_and_refreshes() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueState::new(
            vec![
                Song { id: Some(1), ..create_test_song("song1") },
                Song { id: Some(2), ..create_test_song("song2") },
            ],
            tx,
            daemon.clone(),
        );

        store.move_id(1, 1);

        assert_eq!(store.get(0).and_then(|song| song.id), Some(2));
        assert_eq!(store.get(1).and_then(|song| song.id), Some(1));
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.move_count.load(Ordering::SeqCst), 1);
        assert_eq!(daemon.refresh_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_play_requests_refresh_after_optimistic_update() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let daemon = Arc::new(MockDaemon::new());
        let store = QueueState::new(vec![], tx, daemon.clone());
        let song = create_test_song("song1");

        store.play(PlayIntent::Context {
            tracks: vec![song],
            offset: 0,
            shuffle: false,
            source: None,
        });

        assert_eq!(store.len(), 1);
        assert!(matches!(rx.try_recv(), Ok(AppEvent::RequestRender)));
        assert_eq!(daemon.play_with_intent_count.load(Ordering::SeqCst), 1);
        assert_eq!(daemon.refresh_count.load(Ordering::SeqCst), 1);
    }
}
