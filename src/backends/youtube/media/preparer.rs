use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use super::{
    super::{
        audio::{AudioSourcePlan, cache::AudioCache},
        protocol::play_intent::RequestId,
        url_resolver::UrlResolver,
    },
    MediaPreparer,
    PreloadTier,
    PrepareStatus,
    PreparedMedia,
};

static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> RequestId {
    REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

const MAX_PENDING_PRELOADS: usize = 8;

#[derive(Debug)]
pub enum CacheRequest {
    Prepare {
        track_id: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
        response: oneshot::Sender<PrepareResult>,
    },
    Preload {
        track_id: String,
        tier: PreloadTier,
        request_id: RequestId,
    },
    Cancel {
        request_id: RequestId,
    },
    ActivateWindow {
        track_ids: Vec<String>,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PrepareResult {
    StagedPrefix {
        prefix_path: PathBuf,
        prefix_bytes: u64,
        stream_url: String,
        content_length: u64,
    },
    Direct { stream_url: String },
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
enum JobState {
    ResolvingUrl { track_id: String },
    UrlResolved { track_id: String, stream_url: String },
    DownloadingPrefix { track_id: String, stream_url: String },
    Completed(PrepareResult),
}

#[derive(Debug)]
struct InFlightJob {
    state: Mutex<JobState>,
    task: Mutex<Option<JoinHandle<()>>>,
    notify: Notify,
}

impl InFlightJob {
    fn new(track_id: String) -> Self {
        Self {
            state: Mutex::new(JobState::ResolvingUrl { track_id }),
            task: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    fn snapshot(&self) -> JobState {
        self.state.lock().clone()
    }

    fn set_state(&self, new_state: JobState) {
        *self.state.lock() = new_state;
        self.notify.notify_waiters();
    }

    fn replace_task(&self, task: JoinHandle<()>) {
        if let Some(existing) = self.task.lock().replace(task) {
            existing.abort();
        }
    }

    fn clear_task(&self) {
        self.task.lock().take();
    }

    fn cancel(&self) {
        if let Some(task) = self.task.lock().take() {
            task.abort();
        }

        let mut state = self.state.lock();
        if !matches!(*state, JobState::Completed(_)) {
            *state = JobState::Completed(PrepareResult::Cancelled);
            drop(state);
            self.notify.notify_waiters();
        }
    }
}

#[derive(Debug, Clone)]
struct PreloadJob {
    track_id: String,
    tier: PreloadTier,
    request_id: RequestId,
}

#[derive(Debug, Clone)]
struct TierPermits {
    immediate: Arc<Semaphore>,
    gapless: Arc<Semaphore>,
    eager: Arc<Semaphore>,
    background: Arc<Semaphore>,
}

impl TierPermits {
    fn new() -> Self {
        Self {
            immediate: Arc::new(Semaphore::new(2)),
            gapless: Arc::new(Semaphore::new(2)),
            eager: Arc::new(Semaphore::new(2)),
            background: Arc::new(Semaphore::new(1)),
        }
    }

    async fn acquire(&self, tier: PreloadTier) -> OwnedSemaphorePermit {
        match tier {
            PreloadTier::Immediate => {
                Arc::clone(&self.immediate).acquire_owned().await.expect("semaphore closed")
            }
            PreloadTier::Gapless => {
                Arc::clone(&self.gapless).acquire_owned().await.expect("semaphore closed")
            }
            PreloadTier::Eager => {
                Arc::clone(&self.eager).acquire_owned().await.expect("semaphore closed")
            }
            PreloadTier::Background => {
                Arc::clone(&self.background).acquire_owned().await.expect("semaphore closed")
            }
        }
    }
}

#[derive(Debug)]
enum InternalEvent {
    JobFinished { track_id: String },
}

pub struct YouTubeMediaPreparer {
    rx: mpsc::Receiver<CacheRequest>,
    in_flight: HashMap<String, Arc<InFlightJob>>,
    request_to_track: HashMap<RequestId, String>,
    active_window: HashSet<String>,
    url_resolver: Arc<UrlResolver>,
    audio_cache: Arc<AudioCache>,
    audio_source_plan: AudioSourcePlan,
    background_queue: VecDeque<PreloadJob>,
    permits: TierPermits,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
}

#[derive(Clone)]
pub struct YouTubeMediaPreparerHandle {
    tx: mpsc::Sender<CacheRequest>,
}

impl YouTubeMediaPreparer {
    pub fn spawn(
        url_resolver: Arc<UrlResolver>,
        audio_cache: Arc<AudioCache>,
        audio_source_plan: AudioSourcePlan,
    ) -> YouTubeMediaPreparerHandle {
        let (tx, rx) = mpsc::channel(256);
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();

        let mut executor = Self {
            rx,
            in_flight: HashMap::new(),
            request_to_track: HashMap::new(),
            active_window: HashSet::new(),
            url_resolver,
            audio_cache,
            audio_source_plan,
            background_queue: VecDeque::new(),
            permits: TierPermits::new(),
            internal_rx,
            internal_tx,
        };

        tokio::spawn(async move {
            executor.run().await;
        });

        YouTubeMediaPreparerHandle { tx }
    }

    async fn run(&mut self) {
        log::info!("YouTubeMediaPreparer started");

        loop {
            tokio::select! {
                Some(ev) = self.internal_rx.recv() => {
                    match ev {
                        InternalEvent::JobFinished { track_id } => {
                            self.in_flight.remove(&track_id);
                            self.request_to_track.retain(|_, mapped_track_id| mapped_track_id != &track_id);
                        }
                    }
                }

                Some(request) = self.rx.recv() => {
                    match request {
                        CacheRequest::Prepare { track_id, tier, deadline, response } => {
                            self.handle_prepare_request(track_id, tier, deadline, response);
                        }
                        CacheRequest::Preload { track_id, tier, request_id } => {
                            self.handle_preload_request(track_id, tier, request_id);
                        }
                        CacheRequest::Cancel { request_id } => {
                            self.handle_cancel_request(request_id);
                        }
                        CacheRequest::ActivateWindow { track_ids } => {
                            self.handle_activate_window(track_ids);
                        }
                        CacheRequest::Shutdown => {
                            break;
                        }
                    }
                }

                _ = tokio::time::sleep(Duration::from_millis(10)), if !self.background_queue.is_empty() => {
                    self.dispatch_background_job();
                }

                else => {
                    break;
                }
            }
        }

        log::info!("YouTubeMediaPreparer stopped");
    }

    fn handle_prepare_request(
        &mut self,
        track_id: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
        response: oneshot::Sender<PrepareResult>,
    ) {
        log::debug!(
            transport:? = self.audio_source_plan.transport,
            tier:? = tier,
            deadline_ms = deadline.map(|d| d.as_millis() as u64).unwrap_or(0);
            "[PREPARE] Received prepare request"
        );
        self.active_window.insert(track_id.clone());
        let (job, is_new) = self.get_or_create_job(&track_id);

        if is_new {
            self.spawn_url_resolution(track_id.clone(), Arc::clone(&job));
        }

        let audio_cache = Arc::clone(&self.audio_cache);
        let permits = self.permits.clone();
        let internal_tx = self.internal_tx.clone();

        tokio::spawn(async move {
            let result = Self::wait_or_coalesce_impl(
                audio_cache,
                permits,
                internal_tx,
                Arc::clone(&job),
                tier,
                deadline,
            )
            .await;
            let _ = response.send(result);
        });
    }

    fn handle_preload_request(
        &mut self,
        track_id: String,
        tier: PreloadTier,
        request_id: RequestId,
    ) {
        log::debug!(
            transport:? = self.audio_source_plan.transport,
            tier:? = tier,
            request_id = request_id;
            "[PREPARE] Queue preload request"
        );
        self.request_to_track.insert(request_id, track_id.clone());

        if self.in_flight.contains_key(&track_id) {
            return;
        }

        self.drop_queued_track(&track_id);
        let job = PreloadJob { track_id, tier, request_id };

        match job.tier {
            PreloadTier::Immediate | PreloadTier::Gapless | PreloadTier::Eager => {
                self.background_queue.push_front(job);
            }
            PreloadTier::Background => {
                self.background_queue.push_back(job);
            }
        }

        self.trim_background_queue();
    }

    fn handle_cancel_request(&mut self, request_id: RequestId) {
        let before = self.background_queue.len();
        self.background_queue.retain(|job| job.request_id != request_id);
        let removed = before.saturating_sub(self.background_queue.len()) as u64;

        if let Some(track_id) = self.request_to_track.remove(&request_id) {
            let should_cancel = self
                .request_to_track
                .values()
                .all(|existing_track_id| existing_track_id != &track_id);
            if should_cancel {
                self.cancel_track(&track_id);
            }
        }

        log::debug!(
            transport:? = self.audio_source_plan.transport,
            request_id = request_id,
            cancelled_jobs = removed;
            "[PREPARE] Cancel preload request"
        );
    }

    fn handle_activate_window(&mut self, track_ids: Vec<String>) {
        self.active_window = track_ids.into_iter().collect();

        let mut retained = VecDeque::new();
        while let Some(job) = self.background_queue.pop_front() {
            if self.active_window.contains(&job.track_id) {
                retained.push_back(job);
            } else {
                self.request_to_track.remove(&job.request_id);
            }
        }
        self.background_queue = retained;

        let obsolete_tracks: Vec<String> = self
            .in_flight
            .keys()
            .filter(|track_id| !self.active_window.contains(*track_id))
            .cloned()
            .collect();

        for track_id in obsolete_tracks {
            self.cancel_track(&track_id);
        }
    }

    fn dispatch_background_job(&mut self) {
        let Some(job) = self.background_queue.pop_front() else {
            return;
        };

        if !self.active_window.is_empty() && !self.active_window.contains(&job.track_id) {
            self.request_to_track.remove(&job.request_id);
            return;
        }

        let (in_flight, is_new) = self.get_or_create_job(&job.track_id);
        if is_new {
            self.spawn_url_resolution(job.track_id.clone(), Arc::clone(&in_flight));
        }

        let audio_cache = Arc::clone(&self.audio_cache);
        let permits = self.permits.clone();
        let internal_tx = self.internal_tx.clone();
        let track_id = job.track_id;
        let tier = job.tier;

        tokio::spawn(async move {
            let _ = Self::wait_or_coalesce_impl(
                audio_cache,
                permits,
                internal_tx,
                Arc::clone(&in_flight),
                tier,
                None,
            )
            .await;
        });
    }

    fn drop_queued_track(&mut self, track_id: &str) {
        self.background_queue.retain(|job| {
            let keep = job.track_id != track_id;
            if !keep {
                self.request_to_track.remove(&job.request_id);
            }
            keep
        });
    }

    fn trim_background_queue(&mut self) {
        while self.background_queue.len() > MAX_PENDING_PRELOADS {
            let drop_index = self
                .background_queue
                .iter()
                .position(|job| matches!(job.tier, PreloadTier::Background))
                .unwrap_or(self.background_queue.len().saturating_sub(1));

            if let Some(dropped) = self.background_queue.remove(drop_index) {
                self.request_to_track.remove(&dropped.request_id);
                log::debug!(
                    transport:? = self.audio_source_plan.transport,
                    track_id:% = dropped.track_id.as_str(),
                    tier:? = dropped.tier,
                    request_id = dropped.request_id;
                    "[PREPARE] Dropped stale preload due to bounded queue"
                );
            }
        }
    }

    fn cancel_track(&mut self, track_id: &str) {
        if let Some(job) = self.in_flight.remove(track_id) {
            job.cancel();
        }

        self.request_to_track.retain(|_, mapped_track_id| mapped_track_id != track_id);
    }

    fn get_or_create_job(&mut self, track_id: &str) -> (Arc<InFlightJob>, bool) {
        if let Some(existing) = self.in_flight.get(track_id) {
            return (Arc::clone(existing), false);
        }

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        self.in_flight.insert(track_id.to_string(), Arc::clone(&job));
        (job, true)
    }

    fn spawn_url_resolution(&self, track_id: String, job: Arc<InFlightJob>) {
        let url_resolver = Arc::clone(&self.url_resolver);
        let internal_tx = self.internal_tx.clone();
        let uses_local_staging = self.audio_source_plan.uses_local_staging();
        let task_job = Arc::clone(&job);

        let task = tokio::spawn(async move {
            let track_id_for_blocking = track_id.clone();
            let resolved =
                tokio::task::spawn_blocking(move || url_resolver.get_url(&track_id_for_blocking))
                    .await
                    .context("spawn_blocking failed")
                    .and_then(|r| r.context("Failed to resolve stream URL"));

            match resolved {
                Ok(stream_url) => {
                    if !uses_local_staging {
                        log::debug!(
                            prefix_cache_result = "not_required",
                            transport = "direct";
                            "[PREPARE] Local staging disabled, using direct transport"
                        );
                        task_job.set_state(JobState::Completed(PrepareResult::Direct {
                            stream_url,
                        }));
                        task_job.clear_task();
                        let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                    } else {
                        task_job.set_state(JobState::UrlResolved {
                            track_id: track_id.clone(),
                            stream_url,
                        });
                        task_job.clear_task();
                    }
                }
                Err(e) => {
                    task_job.set_state(JobState::Completed(PrepareResult::Failed(e.to_string())));
                    task_job.clear_task();
                    let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                }
            }
        });

        job.replace_task(task);
    }

    #[allow(dead_code)]
    async fn prepare(
        &mut self,
        track_id: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        let (job, is_new) = self.get_or_create_job(&track_id);
        if is_new {
            self.spawn_url_resolution(track_id.clone(), Arc::clone(&job));
        }
        self.wait_or_coalesce(Arc::clone(&job), tier, deadline).await
    }

    async fn wait_or_coalesce(
        &self,
        job: Arc<InFlightJob>,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        Self::wait_or_coalesce_impl(
            Arc::clone(&self.audio_cache),
            self.permits.clone(),
            self.internal_tx.clone(),
            job,
            tier,
            deadline,
        )
        .await
    }

    async fn wait_or_coalesce_impl(
        audio_cache: Arc<AudioCache>,
        permits: TierPermits,
        internal_tx: mpsc::UnboundedSender<InternalEvent>,
        job: Arc<InFlightJob>,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        loop {
            match job.snapshot() {
                JobState::ResolvingUrl { track_id: _ } => {
                    job.notify.notified().await;
                }
                JobState::Completed(result) => {
                    return result;
                }
                JobState::UrlResolved { track_id, stream_url } => {
                    if let Some((prefix_path, prefix_bytes, content_length)) =
                        audio_cache.get_prefix_metadata(&track_id)
                    {
                        log::debug!(
                            tier:? = tier,
                            prefix_cache_result = "hit",
                            content_length = content_length,
                            prefix_bytes = prefix_bytes;
                            "[PREPARE] Prefix cache hit"
                        );
                        return PrepareResult::StagedPrefix {
                            prefix_path,
                            prefix_bytes,
                            stream_url,
                            content_length,
                        };
                    }

                    let should_spawn = {
                        let mut state = job.state.lock();
                        match &*state {
                            JobState::UrlResolved { track_id: current_id, stream_url: current }
                                if current_id == &track_id && current == &stream_url =>
                            {
                                *state = JobState::DownloadingPrefix {
                                    track_id: track_id.clone(),
                                    stream_url: stream_url.clone(),
                                };
                                true
                            }
                            _ => false,
                        }
                    };

                    if should_spawn {
                        Self::spawn_prefix_download(
                            Arc::clone(&audio_cache),
                            permits.clone(),
                            internal_tx.clone(),
                            track_id.clone(),
                            Arc::clone(&job),
                            stream_url.clone(),
                            tier,
                        );
                    }

                    return Self::wait_for_prefix_result(job, stream_url, tier, deadline).await;
                }
                JobState::DownloadingPrefix { track_id: _, stream_url } => {
                    return Self::wait_for_prefix_result(job, stream_url, tier, deadline).await;
                }
            }
        }
    }

    fn spawn_prefix_download(
        audio_cache: Arc<AudioCache>,
        permits: TierPermits,
        internal_tx: mpsc::UnboundedSender<InternalEvent>,
        track_id: String,
        job: Arc<InFlightJob>,
        stream_url: String,
        tier: PreloadTier,
    ) {
        let task_job = Arc::clone(&job);

        let task = tokio::spawn(async move {
            let _permit = permits.acquire(tier).await;

            let outcome = match audio_cache.ensure_prefix(&track_id, &stream_url).await.and_then(
                |(prefix_path, content_length)| {
                    let prefix_bytes = std::fs::metadata(&prefix_path)
                        .with_context(|| {
                            format!("Failed to read prefix metadata for {}", prefix_path.display())
                        })?
                        .len();
                    Ok((prefix_path, prefix_bytes, content_length))
                },
            ) {
                Ok((prefix_path, prefix_bytes, content_length)) => {
                    log::debug!(
                        tier:? = tier,
                        prefix_cache_result = "downloaded",
                        content_length = content_length,
                        prefix_bytes = prefix_bytes;
                        "[PREPARE] Prefix download completed"
                    );
                    PrepareResult::StagedPrefix {
                        prefix_path,
                        prefix_bytes,
                        stream_url: stream_url.clone(),
                        content_length,
                    }
                }
                Err(e) => {
                    log::warn!(
                        tier:? = tier,
                        prefix_cache_result = "failed",
                        error:% = e;
                        "[PREPARE] Prefix download failed"
                    );
                    PrepareResult::Failed(e.to_string())
                }
            };

            task_job.set_state(JobState::Completed(outcome));
            task_job.clear_task();
            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
        });

        job.replace_task(task);
    }

    async fn wait_for_prefix_result(
        job: Arc<InFlightJob>,
        stream_url: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        let wait = async {
            loop {
                match job.snapshot() {
                    JobState::Completed(result) => return result,
                    _ => job.notify.notified().await,
                }
            }
        };

        if tier == PreloadTier::Immediate {
            if let Some(deadline) = deadline {
                match tokio::time::timeout(deadline, wait).await {
                    Ok(PrepareResult::StagedPrefix {
                        prefix_path,
                        prefix_bytes,
                        stream_url: resolved_url,
                        content_length,
                    }) => PrepareResult::StagedPrefix {
                        prefix_path,
                        prefix_bytes,
                        stream_url: resolved_url,
                        content_length,
                    },
                    Ok(PrepareResult::Direct { stream_url: resolved_url }) => {
                        PrepareResult::Direct { stream_url: resolved_url }
                    }
                    Ok(PrepareResult::Cancelled) => PrepareResult::Cancelled,
                    Ok(PrepareResult::Failed(_)) => {
                        log::warn!(
                            tier:? = tier,
                            fallback_reason = "prepare_failed",
                            transport = "direct";
                            "[PREPARE] Immediate tier fallback to direct"
                        );
                        PrepareResult::Direct { stream_url }
                    }
                    Err(_) => {
                        log::warn!(
                            tier:? = tier,
                            fallback_reason = "deadline_timeout",
                            transport = "direct";
                            "[PREPARE] Immediate tier fallback to direct"
                        );
                        PrepareResult::Direct { stream_url }
                    }
                }
            } else {
                match wait.await {
                    PrepareResult::StagedPrefix {
                        prefix_path,
                        prefix_bytes,
                        stream_url: resolved_url,
                        content_length,
                    } => PrepareResult::StagedPrefix {
                        prefix_path,
                        prefix_bytes,
                        stream_url: resolved_url,
                        content_length,
                    },
                    PrepareResult::Direct { stream_url: resolved_url } => {
                        PrepareResult::Direct { stream_url: resolved_url }
                    }
                    PrepareResult::Cancelled => PrepareResult::Cancelled,
                    PrepareResult::Failed(_) => {
                        log::warn!(
                            tier:? = tier,
                            fallback_reason = "prepare_failed_no_deadline",
                            transport = "direct";
                            "[PREPARE] Immediate tier fallback to direct"
                        );
                        PrepareResult::Direct { stream_url }
                    }
                }
            }
        } else {
            wait.await
        }
    }
}

impl YouTubeMediaPreparerHandle {
    pub async fn prepare(
        &self,
        track_id: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> Result<PrepareResult> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(CacheRequest::Prepare { track_id, tier, deadline, response: tx })
            .await
            .context("Failed to send CacheRequest::Prepare")?;
        rx.await.context("YouTubeMediaPreparer dropped prepare response")
    }

    pub fn preload(&self, track_id: String, tier: PreloadTier, request_id: RequestId) {
        if let Err(e) = self.tx.try_send(CacheRequest::Preload { track_id, tier, request_id }) {
            log::debug!("YouTubeMediaPreparer preload dropped: {e}");
        }
    }

    pub fn cancel(&self, request_id: RequestId) {
        if let Err(e) = self.tx.try_send(CacheRequest::Cancel { request_id }) {
            log::debug!("YouTubeMediaPreparer cancel dropped: {e}");
        }
    }

    pub fn activate_playback_window(&self, track_ids: Vec<String>) {
        if let Err(e) = self.tx.try_send(CacheRequest::ActivateWindow { track_ids }) {
            log::debug!("YouTubeMediaPreparer activate_playback_window dropped: {e}");
        }
    }

    pub fn shutdown(&self) {
        if let Err(e) = self.tx.try_send(CacheRequest::Shutdown) {
            log::debug!("YouTubeMediaPreparer shutdown dropped: {e}");
        }
    }
}

fn prepared_media_from_result(result: PrepareResult) -> Result<PreparedMedia> {
    match result {
        PrepareResult::StagedPrefix {
            prefix_path,
            prefix_bytes,
            stream_url,
            content_length,
        } => Ok(PreparedMedia::StagedPrefix {
            path: prefix_path,
            bytes: prefix_bytes,
            url: stream_url,
            content_length,
        }),
        PrepareResult::Direct { stream_url } => Ok(PreparedMedia::Direct { url: stream_url }),
        PrepareResult::Cancelled => Err(anyhow!("Preparation cancelled")),
        PrepareResult::Failed(e) => Err(anyhow!("Preparation failed: {}", e)),
    }
}

#[async_trait]
impl MediaPreparer for YouTubeMediaPreparerHandle {
    async fn prepare(&self, track_id: &str, tier: PreloadTier) -> Result<PreparedMedia> {
        let deadline = match tier {
            PreloadTier::Immediate => Some(Duration::from_secs(5)),
            _ => None,
        };

        let result =
            YouTubeMediaPreparerHandle::prepare(self, track_id.to_string(), tier, deadline).await?;

        prepared_media_from_result(result)
    }

    fn prefetch(&self, track_id: &str, tier: PreloadTier) {
        let request_id = next_request_id();
        self.preload(track_id.to_string(), tier, request_id);
    }

    fn activate_playback_window(&self, track_ids: &[String]) {
        YouTubeMediaPreparerHandle::activate_playback_window(self, track_ids.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::{
        InFlightJob,
        MAX_PENDING_PRELOADS,
        PrepareResult,
        PreloadJob,
        TierPermits,
        YouTubeMediaPreparer,
        prepared_media_from_result,
    };
    use crate::backends::youtube::{
        audio::{AudioCache, AudioSourcePlanner, CacheConfig},
        config::AudioDeliveryMode,
        media::{PreparedMedia, PreloadTier},
        url_resolver::UrlResolver,
    };

    fn build_test_preparer() -> (TempDir, YouTubeMediaPreparer) {
        let temp_dir = TempDir::new().unwrap();
        let cache = Arc::new(
            AudioCache::new(CacheConfig {
                cache_dir: temp_dir.path().to_path_buf(),
                prefix_size: 1024,
                max_cache_size: 1024 * 1024,
            })
            .unwrap(),
        );
        let (tx, rx) = mpsc::channel(16);
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();

        let preparer = YouTubeMediaPreparer {
            rx,
            in_flight: Default::default(),
            request_to_track: Default::default(),
            active_window: Default::default(),
            url_resolver: Arc::new(UrlResolver::default()),
            audio_cache: cache,
            audio_source_plan: AudioSourcePlanner.plan(AudioDeliveryMode::Combined),
            background_queue: Default::default(),
            permits: TierPermits::new(),
            internal_rx,
            internal_tx,
        };

        drop(tx);
        (temp_dir, preparer)
    }

    #[test]
    fn staged_prepare_result_preserves_adapter_metadata() {
        let prepared = prepared_media_from_result(PrepareResult::StagedPrefix {
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_bytes: 2048,
            stream_url: "https://example.com/stream".to_string(),
            content_length: 4096,
        })
        .unwrap();

        match prepared {
            PreparedMedia::StagedPrefix { path, bytes, url, content_length } => {
                assert_eq!(path, PathBuf::from("/tmp/prefix.webm"));
                assert_eq!(bytes, 2048);
                assert_eq!(url, "https://example.com/stream");
                assert_eq!(content_length, 4096);
            }
            other => panic!("expected staged prefix, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn activating_new_window_cancels_inflight_and_drops_stale_preloads() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let stale_job = Arc::new(InFlightJob::new("stale".to_string()));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        stale_job.replace_task(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let _ = done_tx.send(());
        }));

        preparer.in_flight.insert("stale".to_string(), Arc::clone(&stale_job));
        preparer.request_to_track.insert(1, "stale-queued".to_string());
        preparer.request_to_track.insert(2, "keep".to_string());
        preparer.background_queue.push_back(PreloadJob {
            track_id: "stale-queued".to_string(),
            tier: PreloadTier::Background,
            request_id: 1,
        });
        preparer.background_queue.push_back(PreloadJob {
            track_id: "keep".to_string(),
            tier: PreloadTier::Gapless,
            request_id: 2,
        });

        preparer.handle_activate_window(vec!["keep".to_string()]);

        assert!(!preparer.in_flight.contains_key("stale"));
        assert!(matches!(stale_job.snapshot(), super::JobState::Completed(PrepareResult::Cancelled)));
        assert_eq!(preparer.background_queue.len(), 1);
        assert_eq!(preparer.background_queue[0].track_id, "keep");
        assert!(!preparer.request_to_track.contains_key(&1));
        assert!(preparer.request_to_track.contains_key(&2));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(50), done_rx).await,
            Ok(Err(_))
        ));
    }

    #[tokio::test]
    async fn cancel_request_aborts_inflight_preload() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let stale_job = Arc::new(InFlightJob::new("stale".to_string()));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        stale_job.replace_task(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let _ = done_tx.send(());
        }));

        preparer.in_flight.insert("stale".to_string(), Arc::clone(&stale_job));
        preparer.request_to_track.insert(41, "stale".to_string());

        preparer.handle_cancel_request(41);

        assert!(!preparer.in_flight.contains_key("stale"));
        assert!(matches!(stale_job.snapshot(), super::JobState::Completed(PrepareResult::Cancelled)));
        assert!(!preparer.request_to_track.contains_key(&41));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(50), done_rx).await,
            Ok(Err(_))
        ));
    }

    #[tokio::test]
    async fn cancel_request_keeps_shared_track_alive_until_last_request_is_removed() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let shared_job = Arc::new(InFlightJob::new("shared".to_string()));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        shared_job.replace_task(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let _ = done_tx.send(());
        }));

        preparer.in_flight.insert("shared".to_string(), Arc::clone(&shared_job));
        preparer.request_to_track.insert(41, "shared".to_string());
        preparer.request_to_track.insert(42, "shared".to_string());

        preparer.handle_cancel_request(41);

        assert!(preparer.in_flight.contains_key("shared"));
        assert!(!preparer.request_to_track.contains_key(&41));
        assert!(preparer.request_to_track.contains_key(&42));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(50), &mut Box::pin(done_rx)).await,
            Err(_)
        ));

        preparer.handle_cancel_request(42);

        assert!(!preparer.in_flight.contains_key("shared"));
        assert!(!preparer.request_to_track.contains_key(&42));
        assert!(matches!(shared_job.snapshot(), super::JobState::Completed(PrepareResult::Cancelled)));
    }

    #[test]
    fn preload_queue_is_bounded_and_keeps_latest_background_jobs() {
        let (_temp_dir, mut preparer) = build_test_preparer();

        for idx in 0..(MAX_PENDING_PRELOADS + 3) {
            preparer.handle_preload_request(
                format!("track-{idx}"),
                PreloadTier::Background,
                idx as u64 + 1,
            );
        }

        assert_eq!(preparer.background_queue.len(), MAX_PENDING_PRELOADS);

        let queued: Vec<String> = preparer
            .background_queue
            .iter()
            .map(|job| job.track_id.clone())
            .collect();
        let expected: Vec<String> = (3..(MAX_PENDING_PRELOADS + 3))
            .map(|idx| format!("track-{idx}"))
            .collect();

        assert_eq!(queued, expected);
        assert!(!preparer.request_to_track.contains_key(&1));
        assert!(!preparer.request_to_track.contains_key(&2));
        assert!(!preparer.request_to_track.contains_key(&3));
        assert!(preparer.request_to_track.contains_key(&(MAX_PENDING_PRELOADS as u64 + 3)));
    }

    #[test]
    fn preload_queue_retains_priority_jobs_under_background_churn() {
        let (_temp_dir, mut preparer) = build_test_preparer();

        for idx in 0..MAX_PENDING_PRELOADS {
            preparer.handle_preload_request(
                format!("background-{idx}"),
                PreloadTier::Background,
                idx as u64 + 1,
            );
        }

        preparer.handle_preload_request(
            "gapless-priority".to_string(),
            PreloadTier::Gapless,
            99,
        );
        preparer.handle_preload_request(
            "immediate-priority".to_string(),
            PreloadTier::Immediate,
            100,
        );

        let queued: Vec<(String, PreloadTier)> = preparer
            .background_queue
            .iter()
            .map(|job| (job.track_id.clone(), job.tier))
            .collect();

        assert_eq!(preparer.background_queue.len(), MAX_PENDING_PRELOADS);
        assert!(queued.contains(&("gapless-priority".to_string(), PreloadTier::Gapless)));
        assert!(queued.contains(&("immediate-priority".to_string(), PreloadTier::Immediate)));
        assert!(!preparer.request_to_track.contains_key(&1));
        assert!(!preparer.request_to_track.contains_key(&2));
        assert!(preparer.request_to_track.contains_key(&99));
        assert!(preparer.request_to_track.contains_key(&100));
    }
}
