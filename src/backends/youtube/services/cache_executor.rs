use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tokio::sync::{
    mpsc,
    oneshot,
    Notify,
    OwnedSemaphorePermit,
    Semaphore,
};

use super::super::{
    audio::cache::AudioCache,
    protocol::play_intent::{PreloadTier, RequestId},
    url_resolver::UrlResolver,
};

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
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PrepareResult {
    Concat {
        prefix_path: PathBuf,
        stream_url: String,
        content_length: u64,
    },
    Passthrough {
        stream_url: String,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
enum JobState {
    ResolvingUrl {
        track_id: String,
    },
    UrlResolved {
        track_id: String,
        stream_url: String,
    },
    DownloadingPrefix {
        track_id: String,
        stream_url: String,
    },
    Completed(PrepareResult),
}

#[derive(Debug)]
struct InFlightJob {
    state: Mutex<JobState>,
    notify: Notify,
}

impl InFlightJob {
    fn new(track_id: String) -> Self {
        Self {
            state: Mutex::new(JobState::ResolvingUrl { track_id }),
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
            PreloadTier::Immediate => Arc::clone(&self.immediate)
                .acquire_owned()
                .await
                .expect("semaphore closed"),
            PreloadTier::Gapless => Arc::clone(&self.gapless)
                .acquire_owned()
                .await
                .expect("semaphore closed"),
            PreloadTier::Eager => Arc::clone(&self.eager)
                .acquire_owned()
                .await
                .expect("semaphore closed"),
            PreloadTier::Background => Arc::clone(&self.background)
                .acquire_owned()
                .await
                .expect("semaphore closed"),
        }
    }
}

#[derive(Debug)]
enum InternalEvent {
    JobFinished { track_id: String },
}

pub struct CacheExecutor {
    rx: mpsc::Receiver<CacheRequest>,
    in_flight: HashMap<String, Arc<InFlightJob>>,
    url_resolver: Arc<UrlResolver>,
    audio_cache: Arc<AudioCache>,
    background_queue: VecDeque<PreloadJob>,
    permits: TierPermits,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
}

#[derive(Clone)]
pub struct CacheExecutorHandle {
    tx: mpsc::Sender<CacheRequest>,
}

impl CacheExecutor {
    pub fn spawn(url_resolver: Arc<UrlResolver>, audio_cache: Arc<AudioCache>) -> CacheExecutorHandle {
        let (tx, rx) = mpsc::channel(256);
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();

        let mut executor = Self {
            rx,
            in_flight: HashMap::new(),
            url_resolver,
            audio_cache,
            background_queue: VecDeque::new(),
            permits: TierPermits::new(),
            internal_rx,
            internal_tx,
        };

        tokio::spawn(async move {
            executor.run().await;
        });

        CacheExecutorHandle { tx }
    }

    async fn run(&mut self) {
        log::info!("CacheExecutor started");

        loop {
            tokio::select! {
                Some(ev) = self.internal_rx.recv() => {
                    match ev {
                        InternalEvent::JobFinished { track_id } => {
                            self.in_flight.remove(&track_id);
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

        log::info!("CacheExecutor stopped");
    }

    fn handle_prepare_request(
        &mut self,
        track_id: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
        response: oneshot::Sender<PrepareResult>,
    ) {
        let (job, is_new) = self.get_or_create_job(&track_id);

        if is_new {
            self.spawn_url_resolution(track_id.clone(), Arc::clone(&job));
        }

        let url_resolver = Arc::clone(&self.url_resolver);
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

    fn handle_preload_request(&mut self, track_id: String, tier: PreloadTier, request_id: RequestId) {
        let job = PreloadJob { track_id, tier, request_id };

        match job.tier {
            PreloadTier::Immediate | PreloadTier::Gapless | PreloadTier::Eager => {
                self.background_queue.push_front(job);
            }
            PreloadTier::Background => {
                self.background_queue.push_back(job);
            }
        }
    }

    fn handle_cancel_request(&mut self, request_id: RequestId) {
        self.background_queue.retain(|job| job.request_id != request_id);
    }

    fn dispatch_background_job(&mut self) {
        let Some(job) = self.background_queue.pop_front() else {
            return;
        };

        let (in_flight, is_new) = self.get_or_create_job(&job.track_id);
        if is_new {
            self.spawn_url_resolution(job.track_id.clone(), Arc::clone(&in_flight));
        }

        let url_resolver = Arc::clone(&self.url_resolver);
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

        tokio::spawn(async move {
            let track_id_for_blocking = track_id.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                url_resolver.get_stream_info(&track_id_for_blocking)
            })
            .await
            .context("spawn_blocking failed")
            .and_then(|r| r.context("Failed to resolve stream info"));

            match resolved {
                Ok(info) => {
                    job.set_state(JobState::UrlResolved { track_id: track_id.clone(), stream_url: info.url });
                }
                Err(e) => {
                    job.set_state(JobState::Completed(PrepareResult::Failed(e.to_string())));
                    let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                }
            }
        });
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
                    if audio_cache.has_prefix(&track_id) {
                        if let Some(content_length) = audio_cache.get_content_length(&track_id) {
                            let prefix_path = audio_cache.cache_path(&track_id);
                            return PrepareResult::Concat { prefix_path, stream_url, content_length };
                        }
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
        tokio::spawn(async move {
            let _permit = permits.acquire(tier).await;

            let outcome = match audio_cache.ensure_prefix(&track_id, &stream_url).await {
                Ok((prefix_path, content_length)) => {
                    PrepareResult::Concat { prefix_path, stream_url: stream_url.clone(), content_length }
                }
                Err(e) => PrepareResult::Failed(e.to_string()),
            };

            job.set_state(JobState::Completed(outcome));
            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
        });
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
                    Ok(PrepareResult::Concat { prefix_path, stream_url: resolved_url, content_length }) => {
                        PrepareResult::Concat { prefix_path, stream_url: resolved_url, content_length }
                    }
                    Ok(PrepareResult::Passthrough { stream_url: resolved_url }) => {
                        PrepareResult::Passthrough { stream_url: resolved_url }
                    }
                    Ok(PrepareResult::Failed(_)) | Err(_) => PrepareResult::Passthrough { stream_url },
                }
            } else {
                match wait.await {
                    PrepareResult::Concat { prefix_path, stream_url: resolved_url, content_length } => {
                        PrepareResult::Concat { prefix_path, stream_url: resolved_url, content_length }
                    }
                    PrepareResult::Passthrough { stream_url: resolved_url } => {
                        PrepareResult::Passthrough { stream_url: resolved_url }
                    }
                    PrepareResult::Failed(_) => PrepareResult::Passthrough { stream_url },
                }
            }
        } else {
            wait.await
        }
    }
}

impl CacheExecutorHandle {
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
        rx.await.context("CacheExecutor dropped prepare response")
    }

    pub fn preload(&self, track_id: String, tier: PreloadTier, request_id: RequestId) {
        if let Err(e) = self.tx.try_send(CacheRequest::Preload { track_id, tier, request_id }) {
            log::debug!("CacheExecutor preload dropped: {e}");
        }
    }

    pub fn cancel(&self, request_id: RequestId) {
        if let Err(e) = self.tx.try_send(CacheRequest::Cancel { request_id }) {
            log::debug!("CacheExecutor cancel dropped: {e}");
        }
    }

    pub fn shutdown(&self) {
        if let Err(e) = self.tx.try_send(CacheRequest::Shutdown) {
            log::debug!("CacheExecutor shutdown dropped: {e}");
        }
    }
}
