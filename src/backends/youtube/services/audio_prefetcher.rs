//! Rate-limited audio prefetcher with priority queue.
#![allow(dead_code)]
#![deprecated(since = "0.1.0", note = "Use PreloadScheduler instead")]

//! Downloads audio data for upcoming tracks with proper rate limiting
//! to avoid YouTube rate limiting/bans. Prioritizes tracks closest
//! to current playback position.
//!
//! # Architecture
//!
//! ```text
//! QueueEvent::ItemsAdded ─┐
//!                         ├─> AudioPrefetcher.queue_batch()
//! QueueEvent::OrderChanged┘           │
//!                                     ▼
//!                            ┌─────────────────┐
//!                            │ Priority Queue  │ (sorted by distance from current)
//!                            │ [id:1, id:3, …] │
//!                            └────────┬────────┘
//!                                     │
//!                                     ▼
//!                            ┌─────────────────┐
//!                            │ Rate Limiter    │ (1 req/sec)
//!                            │ Token Bucket    │
//!                            └────────┬────────┘
//!                                     │
//!                                     ▼
//!                            ┌─────────────────┐
//!                            │AudioFileManager │
//!                            │ .prefetch()     │
//!                            └─────────────────┘
//! ```

use std::{
    collections::{BinaryHeap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam::channel::{self, Receiver, Sender};
use parking_lot::Mutex;

/// Commands sent to the prefetch worker thread.
#[derive(Debug)]
enum PrefetchCommand {
    /// Queue new video IDs for prefetching.
    Queue(Vec<PrefetchRequest>),
    /// Cancel prefetch for these video IDs.
    Cancel(Vec<String>),
    /// Update context (current track and play order) for priority calculation.
    UpdateContext { current_id: Option<String>, play_order: Vec<String> },
    /// Shutdown the worker.
    Shutdown,
}

/// A single prefetch request with its video ID.
#[derive(Debug, Clone)]
struct PrefetchRequest {
    video_id: String,
}

/// Task in the priority queue.
#[derive(Debug, Clone, Eq, PartialEq)]
struct PrefetchTask {
    video_id: String,
    /// Priority: lower = higher priority (distance from current track).
    priority: u32,
}

impl Ord for PrefetchTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse for min-heap behavior (lower priority number = higher in queue)
        other.priority.cmp(&self.priority)
    }
}

impl PartialOrd for PrefetchTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Configuration for the audio prefetcher.
#[derive(Debug, Clone)]
pub struct AudioPrefetcherConfig {
    /// Minimum delay between requests (rate limiting).
    pub min_delay: Duration,
    /// Maximum concurrent prefetch operations (currently 1 for rate limiting).
    pub max_concurrent: usize,
}

impl Default for AudioPrefetcherConfig {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(1000), // 1 req/sec
            max_concurrent: 1,
        }
    }
}

/// Handle to control the prefetcher from outside.
#[derive(Clone)]
pub struct AudioPrefetcherHandle {
    cmd_tx: Sender<PrefetchCommand>,
    running: Arc<AtomicBool>,
}

impl AudioPrefetcherHandle {
    /// Queue video IDs for prefetching.
    pub fn queue_batch(&self, video_ids: Vec<String>) {
        if video_ids.is_empty() {
            return;
        }

        let requests: Vec<_> =
            video_ids.into_iter().map(|video_id| PrefetchRequest { video_id }).collect();

        if let Err(e) = self.cmd_tx.send(PrefetchCommand::Queue(requests)) {
            log::warn!("Failed to send prefetch queue command: {}", e);
        }
    }

    /// Cancel prefetch for removed video IDs.
    pub fn cancel(&self, video_ids: Vec<String>) {
        if video_ids.is_empty() {
            return;
        }

        if let Err(e) = self.cmd_tx.send(PrefetchCommand::Cancel(video_ids)) {
            log::warn!("Failed to send prefetch cancel command: {}", e);
        }
    }

    /// Update playback context for priority calculation.
    pub fn update_context(&self, current_id: Option<String>, play_order: Vec<String>) {
        if let Err(e) = self.cmd_tx.send(PrefetchCommand::UpdateContext { current_id, play_order })
        {
            log::warn!("Failed to send prefetch context update: {}", e);
        }
    }

    /// Check if the prefetcher is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Shutdown the prefetcher.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(PrefetchCommand::Shutdown);
    }
}

/// Callback trait for performing the actual prefetch operation.
pub trait PrefetchCallback: Send + Sync + 'static {
    /// Check if audio is already cached for this video ID.
    fn is_cached(&self, video_id: &str) -> bool;

    /// Perform the prefetch operation (resolve URL + download audio).
    fn prefetch(&self, video_id: &str);
}

pub struct PlaybackServiceCallback {
    playback: Arc<super::PlaybackService>,
}

impl PlaybackServiceCallback {
    pub fn new(playback: Arc<super::PlaybackService>) -> Self {
        Self { playback }
    }
}

impl PrefetchCallback for PlaybackServiceCallback {
    fn is_cached(&self, video_id: &str) -> bool {
        self.playback.has_cached_audio(video_id)
    }

    fn prefetch(&self, video_id: &str) {
        self.playback.prefetch(vec![video_id.to_string()]);
    }
}

/// Rate-limited audio prefetcher with priority queue.
pub struct AudioPrefetcher {
    handle: AudioPrefetcherHandle,
    worker_handle: Option<JoinHandle<()>>,
}

impl AudioPrefetcher {
    /// Create and start a new audio prefetcher.
    pub fn new<C: PrefetchCallback>(config: AudioPrefetcherConfig, callback: C) -> Self {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));

        let handle = AudioPrefetcherHandle { cmd_tx, running: Arc::clone(&running) };

        let worker_handle = Self::spawn_worker(config, cmd_rx, Arc::clone(&running), callback);

        Self { handle, worker_handle: Some(worker_handle) }
    }

    /// Get a handle to control the prefetcher.
    pub fn handle(&self) -> AudioPrefetcherHandle {
        self.handle.clone()
    }

    fn spawn_worker<C: PrefetchCallback>(
        config: AudioPrefetcherConfig,
        cmd_rx: Receiver<PrefetchCommand>,
        running: Arc<AtomicBool>,
        callback: C,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut worker = PrefetchWorker::new(config, callback);
            worker.run(cmd_rx, running);
        })
    }
}

impl Drop for AudioPrefetcher {
    fn drop(&mut self) {
        self.handle.shutdown();

        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Internal worker state.
struct PrefetchWorker<C: PrefetchCallback> {
    config: AudioPrefetcherConfig,
    callback: C,

    /// Priority queue of pending tasks.
    work_queue: BinaryHeap<PrefetchTask>,
    /// Set of video IDs currently in queue or in-flight.
    pending_ids: HashSet<String>,
    /// Video IDs that have been cancelled.
    cancelled_ids: HashSet<String>,

    /// Rate limiting: last request timestamp.
    last_request: Option<Instant>,

    /// Current playback context for priority calculation.
    current_id: Option<String>,
    play_order: Vec<String>,
}

impl<C: PrefetchCallback> PrefetchWorker<C> {
    fn new(config: AudioPrefetcherConfig, callback: C) -> Self {
        Self {
            config,
            callback,
            work_queue: BinaryHeap::new(),
            pending_ids: HashSet::new(),
            cancelled_ids: HashSet::new(),
            last_request: None,
            current_id: None,
            play_order: Vec::new(),
        }
    }

    fn run(&mut self, cmd_rx: Receiver<PrefetchCommand>, running: Arc<AtomicBool>) {
        log::info!("Audio prefetcher worker started");

        while running.load(Ordering::SeqCst) {
            // Process commands with timeout to allow periodic work processing
            match cmd_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(cmd) => {
                    if !self.handle_command(cmd) {
                        break; // Shutdown requested
                    }
                }
                Err(channel::RecvTimeoutError::Timeout) => {
                    // No command, try to process work
                }
                Err(channel::RecvTimeoutError::Disconnected) => {
                    log::info!("Prefetcher command channel disconnected");
                    break;
                }
            }

            // Process one work item if rate limit allows
            self.process_one_task();
        }

        running.store(false, Ordering::SeqCst);
        log::info!("Audio prefetcher worker stopped");
    }

    fn handle_command(&mut self, cmd: PrefetchCommand) -> bool {
        match cmd {
            PrefetchCommand::Queue(requests) => {
                self.queue_requests(requests);
                true
            }
            PrefetchCommand::Cancel(video_ids) => {
                self.cancel_requests(video_ids);
                true
            }
            PrefetchCommand::UpdateContext { current_id, play_order } => {
                self.update_context(current_id, play_order);
                true
            }
            PrefetchCommand::Shutdown => {
                log::info!("Prefetcher shutdown requested");
                false
            }
        }
    }

    fn queue_requests(&mut self, requests: Vec<PrefetchRequest>) {
        for req in requests {
            // Skip if already pending or already cached
            if self.pending_ids.contains(&req.video_id) {
                continue;
            }
            if self.callback.is_cached(&req.video_id) {
                log::debug!("Skipping already cached: {}", req.video_id);
                continue;
            }

            self.cancelled_ids.remove(&req.video_id);

            let priority = self.calculate_priority(&req.video_id);
            self.pending_ids.insert(req.video_id.clone());
            self.work_queue.push(PrefetchTask { video_id: req.video_id, priority });
        }

        log::debug!(
            "Prefetch queue size: {}, pending: {}",
            self.work_queue.len(),
            self.pending_ids.len()
        );
    }

    fn cancel_requests(&mut self, video_ids: Vec<String>) {
        for id in video_ids {
            self.cancelled_ids.insert(id.clone());
            self.pending_ids.remove(&id);
        }

        // Note: We can't easily remove from BinaryHeap, so we mark as cancelled
        // and skip when processing
        log::debug!("Cancelled {} prefetch requests", self.cancelled_ids.len());
    }

    fn update_context(&mut self, current_id: Option<String>, play_order: Vec<String>) {
        let context_changed =
            self.current_id != current_id || self.play_order.len() != play_order.len();

        self.current_id = current_id;
        self.play_order = play_order;

        if context_changed {
            self.reprioritize();
        }
    }

    fn calculate_priority(&self, video_id: &str) -> u32 {
        // Priority = distance from current track in play order
        // Lower number = higher priority

        let current_pos = self
            .current_id
            .as_ref()
            .and_then(|id| self.play_order.iter().position(|x| x == id))
            .unwrap_or(0);

        let id_pos = self.play_order.iter().position(|x| x == video_id);

        match id_pos {
            Some(pos) if pos >= current_pos => {
                // Ahead of current: priority = distance
                (pos - current_pos) as u32
            }
            Some(_) => {
                // Behind current: low priority (wrap-around for repeat)
                u32::MAX / 2
            }
            None => {
                // Not in play order: lowest priority
                u32::MAX
            }
        }
    }

    fn reprioritize(&mut self) {
        // Rebuild queue with new priorities
        let tasks: Vec<_> = self.work_queue.drain().collect();

        for task in tasks {
            // Skip cancelled
            if self.cancelled_ids.contains(&task.video_id) {
                self.pending_ids.remove(&task.video_id);
                continue;
            }

            let new_priority = self.calculate_priority(&task.video_id);
            self.work_queue.push(PrefetchTask { video_id: task.video_id, priority: new_priority });
        }

        log::debug!("Reprioritized {} tasks", self.work_queue.len());
    }

    fn process_one_task(&mut self) {
        // Check rate limit
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.config.min_delay {
                return; // Rate limited, wait
            }
        }

        // Find next valid task
        while let Some(task) = self.work_queue.pop() {
            // Skip cancelled
            if self.cancelled_ids.remove(&task.video_id) {
                self.pending_ids.remove(&task.video_id);
                continue;
            }

            // Skip already cached
            if self.callback.is_cached(&task.video_id) {
                self.pending_ids.remove(&task.video_id);
                continue;
            }

            // Process this task
            log::debug!("Prefetching {} (priority {})", task.video_id, task.priority);

            self.callback.prefetch(&task.video_id);
            self.pending_ids.remove(&task.video_id);
            self.last_request = Some(Instant::now());

            return; // Only process one per cycle
        }
    }
}

/// Statistics about the prefetcher state.
#[derive(Debug, Clone)]
pub struct AudioPrefetcherStats {
    pub queue_size: usize,
    pub pending_count: usize,
    pub is_running: bool,
}

impl AudioPrefetcher {
    /// Get current prefetcher statistics.
    pub fn stats(&self) -> AudioPrefetcherStats {
        AudioPrefetcherStats {
            queue_size: 0,    // Would need shared state to expose
            pending_count: 0, // Would need shared state to expose
            is_running: self.handle.is_running(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    struct MockCallback {
        cached: Mutex<HashSet<String>>,
        prefetch_count: AtomicUsize,
        prefetched: Mutex<Vec<String>>,
    }

    impl MockCallback {
        fn new() -> Self {
            Self {
                cached: Mutex::new(HashSet::new()),
                prefetch_count: AtomicUsize::new(0),
                prefetched: Mutex::new(Vec::new()),
            }
        }
    }

    impl PrefetchCallback for Arc<MockCallback> {
        fn is_cached(&self, video_id: &str) -> bool {
            self.cached.lock().contains(video_id)
        }

        fn prefetch(&self, video_id: &str) {
            self.prefetch_count.fetch_add(1, Ordering::SeqCst);
            self.prefetched.lock().push(video_id.to_string());
            self.cached.lock().insert(video_id.to_string());
        }
    }

    #[test]
    fn test_priority_calculation() {
        let callback = Arc::new(MockCallback::new());
        let config = AudioPrefetcherConfig {
            min_delay: Duration::from_millis(10), // Fast for testing
            max_concurrent: 1,
        };

        let prefetcher = AudioPrefetcher::new(config, Arc::clone(&callback));
        let handle = prefetcher.handle();

        // Set up play order: a, b, c, d, e with c as current
        handle.update_context(Some("c".to_string()), vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ]);

        // Queue d and e (after current), should be prioritized in order
        handle.queue_batch(vec!["d".to_string(), "e".to_string()]);

        // Wait for processing
        thread::sleep(Duration::from_millis(100));

        let prefetched = callback.prefetched.lock();
        assert!(!prefetched.is_empty());
        // d should come before e (closer to current)
        if prefetched.len() >= 2 {
            assert_eq!(prefetched[0], "d");
            assert_eq!(prefetched[1], "e");
        }
    }

    #[test]
    fn test_cancellation() {
        let callback = Arc::new(MockCallback::new());
        let config = AudioPrefetcherConfig {
            min_delay: Duration::from_millis(500), // Slow to allow cancellation
            max_concurrent: 1,
        };

        let prefetcher = AudioPrefetcher::new(config, Arc::clone(&callback));
        let handle = prefetcher.handle();

        // Queue some items
        handle.queue_batch(vec!["x".to_string(), "y".to_string(), "z".to_string()]);

        // Immediately cancel y
        handle.cancel(vec!["y".to_string()]);

        // Wait for processing
        thread::sleep(Duration::from_millis(1500));

        let prefetched = callback.prefetched.lock();
        assert!(!prefetched.contains(&"y".to_string()), "y should have been cancelled");
    }

    #[test]
    fn test_skip_cached() {
        let callback = Arc::new(MockCallback::new());

        // Pre-cache one item
        callback.cached.lock().insert("cached_item".to_string());

        let config =
            AudioPrefetcherConfig { min_delay: Duration::from_millis(10), max_concurrent: 1 };

        let prefetcher = AudioPrefetcher::new(config, Arc::clone(&callback));
        let handle = prefetcher.handle();

        // Queue cached and uncached items
        handle.queue_batch(vec!["cached_item".to_string(), "new_item".to_string()]);

        // Wait for processing
        thread::sleep(Duration::from_millis(100));

        let prefetched = callback.prefetched.lock();
        assert!(!prefetched.contains(&"cached_item".to_string()));
        assert!(prefetched.contains(&"new_item".to_string()));
    }
}
