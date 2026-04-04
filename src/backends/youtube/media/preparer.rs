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
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use super::{
    super::{
        audio::{AudioDeliveryPlan, cache::AudioCache},
        config::BackgroundExtractMode,
        protocol::play_intent::RequestId,
        url_resolver::UrlResolver,
    },
    ExtractRegistryState, MediaPreparationPlan, MediaPreparer, PreloadTier, PrepareStatus,
    PreparedMedia, StreamResolver,
};
use super::{
    job_registry::{
        InFlightJob, JobProgress, JobRegistry, ResolutionApplyOutcome, ResolutionSource,
    },
    staging_pipeline::StagingPipeline,
};

static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static TRACE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> RequestId {
    REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn next_trace_id() -> u64 {
    TRACE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

const MAX_PENDING_PRELOADS: usize = 8;

#[cfg(test)]
const SHARED_JOB_JOIN_TIMEOUT: Duration = Duration::from_millis(50);

#[cfg(not(test))]
const SHARED_JOB_JOIN_TIMEOUT: Duration = Duration::from_secs(6);

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
    Warm {
        track_id: String,
    },
    WarmBatch {
        track_ids: Vec<String>,
    },
    Cancel {
        request_id: RequestId,
    },
    ActivateWindow {
        track_ids: Vec<String>,
    },
    ApplyPlan {
        plan: MediaPreparationPlan,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareResult {
    StagedPrefix {
        prefix_path: PathBuf,
        prefix_bytes: u64,
        stream_url: String,
        content_length: u64,
    },
    /// Relay will stream from byte 0 in a single connection, teeing first
    /// prefix_size bytes to prefix_path.  Used on cache miss to avoid a
    /// separate prefix download that triggers YouTube CDN rate-limiting.
    StreamAndCache {
        stream_url: String,
        content_length: u64,
        prefix_path: PathBuf,
        prefix_size: u64,
    },
    Direct {
        stream_url: String,
    },
    Cancelled,
    Failed(String),
}

/// Parse the `clen=` query parameter from a YouTube stream URL.
fn parse_content_length_from_url(url: &str) -> Option<u64> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .find_map(|param| param.strip_prefix("clen="))
        .and_then(|v| v.parse::<u64>().ok())
}

#[derive(Debug, Clone)]
enum UrlResolvedAction {
    Complete(PrepareResult),
    WaitForPrefixDownload { track_id: String, stream_url: String },
}

fn decide_url_resolved_action(
    pipeline: &StagingPipeline,
    allow_immediate_direct: bool,
    stream_immediate_cache_miss: bool,
    tier: PreloadTier,
    track_id: &str,
    stream_url: &str,
) -> UrlResolvedAction {
    if let Some((prefix_path, prefix_bytes, content_length)) =
        pipeline.get_prefix_metadata(track_id)
    {
        log::debug!(
            "[PREPARE] Prefix cache hit track_id={} tier={:?} prefix_bytes={} content_length={}",
            track_id,
            tier,
            prefix_bytes,
            content_length,
        );

        return UrlResolvedAction::Complete(PrepareResult::StagedPrefix {
            prefix_path,
            prefix_bytes,
            stream_url: stream_url.to_string(),
            content_length,
        });
    }

    if tier == PreloadTier::Immediate {
        if stream_immediate_cache_miss {
            let Some(content_length) = parse_content_length_from_url(stream_url) else {
                return UrlResolvedAction::Complete(PrepareResult::Failed(
                    "Missing clen= in stream URL for tee relay path".to_string(),
                ));
            };

            return UrlResolvedAction::Complete(PrepareResult::StreamAndCache {
                stream_url: stream_url.to_string(),
                content_length,
                prefix_path: pipeline.prefix_path_for(track_id),
                prefix_size: pipeline.default_prefix_size().min(content_length),
            });
        }

        if allow_immediate_direct {
            log::debug!(
                tier:? = tier,
                prefix_cache_result = "miss",
                transport = "direct";
                "[PREPARE] Immediate tier using direct transport on cache miss"
            );

            return UrlResolvedAction::Complete(PrepareResult::Direct {
                stream_url: stream_url.to_string(),
            });
        }
    }

    UrlResolvedAction::WaitForPrefixDownload {
        track_id: track_id.to_string(),
        stream_url: stream_url.to_string(),
    }
}

fn direct_bypass_for_inflight_prefix(
    tier: PreloadTier,
    allow_immediate_direct: bool,
    stream_url: &str,
) -> Option<PrepareResult> {
    if tier == PreloadTier::Immediate && allow_immediate_direct {
        log::debug!(
            tier:? = tier,
            prefix_cache_result = "pending",
            transport = "direct";
            "[PREPARE] Immediate tier bypassing in-flight prefix download"
        );

        return Some(PrepareResult::Direct { stream_url: stream_url.to_string() });
    }

    None
}

fn take_over_queue_warm_with_demand_extract(
    pipeline: &Arc<StagingPipeline>,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    transport: crate::backends::youtube::audio::AudioTransportTarget,
    track_id: &str,
    job: &Arc<InFlightJob>,
    trigger: &'static str,
    tier: PreloadTier,
    reason: &'static str,
) -> bool {
    if job.replace_queue_warm_lease_with_demand().is_none() {
        return false;
    }

    log::debug!(
        track_id = track_id,
        trigger = trigger,
        tier:? = tier,
        reason = reason;
        "[TRACE] Demand prepare taking over queue-warm resolution; stale queue-warm result will be dropped"
    );

    spawn_url_resolution_task(
        Arc::clone(pipeline),
        internal_tx.clone(),
        transport,
        track_id.to_string(),
        Arc::clone(job),
        trigger,
        "extract_one_preempted",
        next_trace_id(),
        ResolutionSource::Demand,
        true,
    );
    true
}

fn spawn_url_resolution_task(
    pipeline: Arc<StagingPipeline>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
    transport: crate::backends::youtube::audio::AudioTransportTarget,
    track_id: String,
    job: Arc<InFlightJob>,
    trigger: &'static str,
    mode: &'static str,
    trace_token: u64,
    resolution_source: ResolutionSource,
    force_fresh: bool,
) {
    let task_job = Arc::clone(&job);
    let task_token = job.allocate_task_token();
    let resolution_token = job.claim_resolution_lease(resolution_source);

    log::debug!(
        "[TRACE] extract_start trace_id={} track_id={} trigger={} mode={} transport={:?}",
        trace_token,
        track_id,
        trigger,
        mode,
        transport,
    );

    let task = tokio::spawn(async move {
        let resolved = if force_fresh {
            pipeline.resolve_stream_url_fresh(track_id.clone()).await
        } else {
            pipeline.resolve_stream_url(track_id.clone()).await
        };

        match resolved {
            Ok(stream_url) => {
                log::info!(
                    "[TRACE] extract_result trace_id={} track_id={} result=ok trigger={} mode={}",
                    trace_token,
                    track_id,
                    trigger,
                    mode,
                );
                match task_job.apply_url_resolution_if_current(
                    resolution_token,
                    track_id.clone(),
                    stream_url,
                    pipeline.uses_local_staging(),
                ) {
                    ResolutionApplyOutcome::Applied { completed } => {
                        if completed {
                            log::debug!(
                                prefix_cache_result = "not_required",
                                transport = "direct";
                                "[PREPARE] Local staging disabled, using direct transport"
                            );
                            task_job.clear_task(task_token);
                            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                        } else {
                            task_job.clear_task(task_token);
                        }
                    }
                    ResolutionApplyOutcome::DroppedStale => {
                        log::debug!(
                            track_id = track_id.as_str(),
                            trigger = trigger,
                            mode = mode,
                            source = "stale_resolution_lease";
                            "[TRACE] Dropping stale extract result"
                        );
                        task_job.clear_task(task_token);
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "[TRACE] extract_result trace_id={} track_id={} result=fail trigger={} mode={} error={}",
                    trace_token,
                    track_id,
                    trigger,
                    mode,
                    e,
                );
                if task_job.fail_if_current(resolution_token, e.to_string()) {
                    task_job.clear_task(task_token);
                    let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                } else {
                    log::debug!(
                        track_id = track_id.as_str(),
                        trigger = trigger,
                        mode = mode,
                        source = "stale_resolution_lease";
                        "[TRACE] Dropping stale extract failure"
                    );
                    task_job.clear_task(task_token);
                }
            }
        }
    });

    job.replace_task(task_token, task);
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
pub(super) enum InternalEvent {
    JobFinished { track_id: String },
}

pub struct YouTubeMediaPreparer {
    rx: mpsc::Receiver<CacheRequest>,
    job_registry: Arc<JobRegistry>,
    request_to_track: HashMap<RequestId, String>,
    active_window: HashSet<String>,
    last_applied_active_window: Option<Vec<String>>,
    last_extract_scope: Option<Vec<String>>,
    last_extract_scope_generation: Option<u64>,
    last_prefix_targets: Option<Vec<String>>,
    pipeline: Arc<StagingPipeline>,
    audio_source_plan: AudioDeliveryPlan,
    background_queue: VecDeque<PreloadJob>,
    permits: TierPermits,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
}

#[derive(Clone)]
pub struct YouTubeMediaPreparerHandle {
    tx: mpsc::Sender<CacheRequest>,
    url_resolver: Arc<dyn StreamResolver>,
    job_registry: Arc<JobRegistry>,
}

impl YouTubeMediaPreparer {
    pub fn spawn(
        url_resolver: Arc<UrlResolver>,
        audio_cache: Arc<AudioCache>,
        audio_source_plan: AudioDeliveryPlan,
    ) -> YouTubeMediaPreparerHandle {
        let (tx, rx) = mpsc::channel(256);
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let resolver_handle: Arc<dyn StreamResolver> = url_resolver.clone();
        let pipeline = Arc::new(StagingPipeline::new(url_resolver, audio_cache, audio_source_plan));
        let job_registry = Arc::new(JobRegistry::default());

        let mut executor = Self {
            rx,
            job_registry: Arc::clone(&job_registry),
            request_to_track: HashMap::new(),
            active_window: HashSet::new(),
            last_applied_active_window: None,
            last_extract_scope: None,
            last_extract_scope_generation: None,
            last_prefix_targets: None,
            pipeline,
            audio_source_plan,
            background_queue: VecDeque::new(),
            permits: TierPermits::new(),
            internal_rx,
            internal_tx,
        };

        tokio::spawn(async move {
            executor.run().await;
        });

        YouTubeMediaPreparerHandle { tx, url_resolver: resolver_handle, job_registry }
    }

    async fn run(&mut self) {
        log::info!("YouTubeMediaPreparer started");

        loop {
            tokio::select! {
                Some(ev) = self.internal_rx.recv() => {
                    match ev {
                        InternalEvent::JobFinished { track_id } => {
                            self.job_registry.remove(&track_id);
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
                CacheRequest::Warm { track_id } => {
                    self.handle_warm_request(track_id);
                }
                CacheRequest::WarmBatch { track_ids } => {
                    self.handle_warm_batch_request(track_ids);
                }
                CacheRequest::Cancel { request_id } => {
                    self.handle_cancel_request(request_id);
                }
                        CacheRequest::ActivateWindow { track_ids } => {
                            self.handle_activate_window(track_ids);
                        }
                        CacheRequest::ApplyPlan { plan } => {
                            self.handle_apply_plan(plan);
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
            "[PREPARE] Received prepare request track_id={} tier={:?} deadline_ms={} transport={:?}",
            track_id,
            tier,
            deadline.map(|d| d.as_millis() as u64).unwrap_or(0),
            self.audio_source_plan.transport,
        );
        self.active_window.insert(track_id.clone());
        let (job, is_new) = self.get_or_create_job(&track_id);

        if is_new {
            self.spawn_url_resolution(
                track_id.clone(),
                Arc::clone(&job),
                "play",
                "extract_one",
                next_trace_id(),
                ResolutionSource::Demand,
            );
        } else {
            log_in_memory_job_state(&track_id, "play", Some(tier), &job.progress());
        }

        let pipeline = Arc::clone(&self.pipeline);
        let permits = self.permits.clone();
        let internal_tx = self.internal_tx.clone();
        let allow_immediate_direct = self.audio_source_plan.allows_immediate_direct();
        let stream_immediate_cache_miss =
            self.audio_source_plan.streams_immediate_cache_miss_via_relay();
        let transport = self.audio_source_plan.transport;
        let prepare_track_id = track_id.clone();

        tokio::spawn(async move {
            let result = Self::wait_or_coalesce_impl(
                pipeline,
                permits,
                internal_tx,
                allow_immediate_direct,
                stream_immediate_cache_miss,
                transport,
                prepare_track_id,
                Arc::clone(&job),
                "play",
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
        if let Some(progress) = self.job_registry.progress(&track_id) {
            log_in_memory_job_state(&track_id, "prefix_window", Some(tier), &progress);
            match progress {
                JobProgress::ResolvingUrl | JobProgress::UrlResolved { .. } => {}
                JobProgress::DownloadingPrefix { .. } | JobProgress::Completed(_) => {
                    return;
                }
            }
        }

        self.drop_queued_track(&track_id);
        self.request_to_track.insert(request_id, track_id.clone());
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

    fn handle_warm_request(&mut self, track_id: String) {
        let (job, is_new) = self.get_or_create_job(&track_id);
        if !is_new {
            log_in_memory_job_state(&track_id, "background_extract", None, &job.progress());
            return;
        }

        let trace_token = next_trace_id();
        log::debug!(
            "[TRACE] extract_start trace_id={} track_id={} trigger=background_extract mode=extract_one transport={:?}",
            trace_token,
            track_id,
            self.audio_source_plan.transport,
        );
        self.spawn_url_resolution(
            track_id,
            job,
            "background_extract",
            "extract_one",
            trace_token,
            ResolutionSource::QueueWarm,
        );
    }

    fn handle_warm_batch_request(&mut self, track_ids: Vec<String>) {
        if track_ids.is_empty() {
            return;
        }

        let pending_jobs: Vec<(String, Arc<InFlightJob>, u64)> = track_ids
            .into_iter()
            .filter_map(|track_id| {
                let (job, is_new) = self.get_or_create_job(&track_id);
                if is_new {
                    let resolution_token = job.claim_resolution_lease(ResolutionSource::QueueWarm);
                    Some((track_id, Arc::clone(&job), resolution_token))
                } else {
                    log_in_memory_job_state(&track_id, "background_extract", None, &job.progress());
                    None
                }
            })
            .collect();

        if pending_jobs.is_empty() {
            return;
        }

        let trace_id = next_trace_id();

        log::debug!(
            "[TRACE] batch_extract_start trace_id={} count={} trigger=background_extract mode=extract_batch transport={:?}",
            trace_id,
            pending_jobs.len(),
            self.audio_source_plan.transport,
        );

        let pipeline = Arc::clone(&self.pipeline);
        let internal_tx = self.internal_tx.clone();
        let uses_local_staging = self.audio_source_plan.uses_local_staging();

        tokio::spawn(async move {
            let track_ids: Vec<String> =
                pending_jobs.iter().map(|(track_id, _, _)| track_id.clone()).collect();
            let resolved = pipeline.resolve_stream_urls(track_ids.clone()).await;

            match resolved {
                Ok(mut results) => {
                    for (track_id, job, resolution_token) in pending_jobs {
                        match results.remove(&track_id) {
                            Some(Ok(stream_url)) => {
                                log::info!(
                                    "[TRACE] extract_result trace_id={} track_id={} result=ok trigger=background_extract mode=extract_batch",
                                    trace_id,
                                    track_id,
                                );

                                match job.apply_url_resolution_if_current(
                                    resolution_token,
                                    track_id.clone(),
                                    stream_url,
                                    uses_local_staging,
                                ) {
                                    ResolutionApplyOutcome::Applied { completed } => {
                                        if completed {
                                            let _ = internal_tx
                                                .send(InternalEvent::JobFinished { track_id });
                                        }
                                    }
                                    ResolutionApplyOutcome::DroppedStale => {
                                        log::debug!(
                                            track_id = track_id.as_str(),
                                            trigger = "background_extract",
                                            mode = "extract_batch",
                                            source = "stale_resolution_lease";
                                            "[TRACE] Dropping stale extract result"
                                        );
                                    }
                                }
                            }
                            Some(Err(error)) => {
                                log::warn!(
                                    "[TRACE] extract_result trace_id={} track_id={} result=fail trigger=background_extract mode=extract_batch error={}",
                                    trace_id,
                                    track_id,
                                    error,
                                );
                                if job.fail_if_current(resolution_token, error.to_string()) {
                                    let _ =
                                        internal_tx.send(InternalEvent::JobFinished { track_id });
                                } else {
                                    log::debug!(
                                        track_id = track_id.as_str(),
                                        trigger = "background_extract",
                                        mode = "extract_batch",
                                        source = "stale_resolution_lease";
                                        "[TRACE] Dropping stale extract failure"
                                    );
                                }
                            }
                            None => {
                                log::warn!(
                                    "[TRACE] extract_result trace_id={} track_id={} result=fail trigger=background_extract mode=extract_batch error=omitted_result",
                                    trace_id,
                                    track_id,
                                );
                                if job
                                    .fail_if_current(resolution_token, "omitted_result".to_string())
                                {
                                    let _ =
                                        internal_tx.send(InternalEvent::JobFinished { track_id });
                                } else {
                                    log::debug!(
                                        track_id = track_id.as_str(),
                                        trigger = "background_extract",
                                        mode = "extract_batch",
                                        source = "stale_resolution_lease";
                                        "[TRACE] Dropping stale extract failure"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    for (track_id, job, resolution_token) in pending_jobs {
                        log::warn!(
                            "[TRACE] extract_result trace_id={} track_id={} result=fail trigger=background_extract mode=extract_batch error={}",
                            trace_id,
                            track_id,
                            error,
                        );
                        if job.fail_if_current(resolution_token, error.to_string()) {
                            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                        } else {
                            log::debug!(
                                track_id = track_id.as_str(),
                                trigger = "background_extract",
                                mode = "extract_batch",
                                source = "stale_resolution_lease";
                                "[TRACE] Dropping stale extract failure"
                            );
                        }
                    }
                }
            }
        });
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
            if should_cancel && !self.track_has_nonrequest_intent(&track_id) {
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
        self.last_applied_active_window = Some(track_ids.clone());
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
    }

    fn handle_apply_plan(&mut self, plan: MediaPreparationPlan) {
        self.apply_active_window(plan.active_window);
        self.apply_prefix_targets(plan.prefix_targets);
        self.apply_extract_scope(
            plan.extract_scope,
            plan.extract_scope_generation,
            plan.background_extract_mode,
        );
    }

    fn apply_active_window(&mut self, track_ids: Vec<String>) {
        if track_ids.is_empty() && self.last_applied_active_window.is_none() {
            return;
        }

        if self.last_applied_active_window.as_ref() == Some(&track_ids) {
            return;
        }

        self.handle_activate_window(track_ids);
    }

    fn apply_extract_scope(
        &mut self,
        extract_scope: Vec<String>,
        generation: u64,
        background_extract_mode: BackgroundExtractMode,
    ) {
        match background_extract_mode {
            BackgroundExtractMode::Balanced => {
                if self.last_extract_scope.as_ref() == Some(&extract_scope) {
                    return;
                }
            }
            BackgroundExtractMode::Performance => {
                if self.last_extract_scope_generation == Some(generation) {
                    return;
                }
            }
        }

        self.last_extract_scope = Some(extract_scope.clone());
        self.last_extract_scope_generation = Some(generation);

        if extract_scope.is_empty() {
            return;
        }

        self.handle_warm_batch_request(extract_scope);
    }

    fn apply_prefix_targets(&mut self, prefix_targets: Vec<String>) {
        if self.last_prefix_targets.as_ref() == Some(&prefix_targets) {
            return;
        }

        self.last_prefix_targets = Some(prefix_targets.clone());

        for track_id in prefix_targets {
            self.handle_preload_request(track_id, PreloadTier::Background, next_request_id());
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
            self.spawn_url_resolution(
                job.track_id.clone(),
                Arc::clone(&in_flight),
                "prefix_window",
                "extract_one",
                next_trace_id(),
                ResolutionSource::Demand,
            );
        }

        let pipeline = Arc::clone(&self.pipeline);
        let permits = self.permits.clone();
        let internal_tx = self.internal_tx.clone();
        let track_id = job.track_id;
        let tier = job.tier;
        let allow_immediate_direct = self.audio_source_plan.allows_immediate_direct();
        let stream_immediate_cache_miss =
            self.audio_source_plan.streams_immediate_cache_miss_via_relay();
        let transport = self.audio_source_plan.transport;

        tokio::spawn(async move {
            let _ = Self::wait_or_coalesce_impl(
                pipeline,
                permits,
                internal_tx,
                allow_immediate_direct,
                stream_immediate_cache_miss,
                transport,
                track_id,
                Arc::clone(&in_flight),
                "prefix_window",
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

    fn track_has_nonrequest_intent(&self, track_id: &str) -> bool {
        self.active_window.contains(track_id)
            || self
                .last_extract_scope
                .as_ref()
                .is_some_and(|track_ids| track_ids.iter().any(|id| id == track_id))
            || self
                .last_prefix_targets
                .as_ref()
                .is_some_and(|track_ids| track_ids.iter().any(|id| id == track_id))
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
        self.job_registry.cancel_track(track_id);

        self.request_to_track.retain(|_, mapped_track_id| mapped_track_id != track_id);
    }

    fn get_or_create_job(&mut self, track_id: &str) -> (Arc<InFlightJob>, bool) {
        self.job_registry.get_or_create(track_id)
    }

    fn spawn_url_resolution(
        &self,
        track_id: String,
        job: Arc<InFlightJob>,
        trigger: &'static str,
        mode: &'static str,
        trace_token: u64,
        resolution_source: ResolutionSource,
    ) {
        spawn_url_resolution_task(
            Arc::clone(&self.pipeline),
            self.internal_tx.clone(),
            self.audio_source_plan.transport,
            track_id,
            job,
            trigger,
            mode,
            trace_token,
            resolution_source,
            false,
        );
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
            self.spawn_url_resolution(
                track_id.clone(),
                Arc::clone(&job),
                "play",
                "extract_one",
                next_trace_id(),
                ResolutionSource::Demand,
            );
        }
        self.wait_or_coalesce(track_id.clone(), Arc::clone(&job), tier, deadline).await
    }

    async fn wait_or_coalesce(
        &self,
        track_id: String,
        job: Arc<InFlightJob>,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        Self::wait_or_coalesce_impl(
            Arc::clone(&self.pipeline),
            self.permits.clone(),
            self.internal_tx.clone(),
            self.audio_source_plan.allows_immediate_direct(),
            self.audio_source_plan.streams_immediate_cache_miss_via_relay(),
            self.audio_source_plan.transport,
            track_id.clone(),
            job,
            "play",
            tier,
            deadline,
        )
        .await
    }

    async fn wait_or_coalesce_impl(
        pipeline: Arc<StagingPipeline>,
        permits: TierPermits,
        internal_tx: mpsc::UnboundedSender<InternalEvent>,
        allow_immediate_direct: bool,
        stream_immediate_cache_miss: bool,
        transport: crate::backends::youtube::audio::AudioTransportTarget,
        track_id: String,
        job: Arc<InFlightJob>,
        trigger: &'static str,
        tier: PreloadTier,
        deadline: Option<Duration>,
    ) -> PrepareResult {
        loop {
            match job.progress() {
                JobProgress::ResolvingUrl => {
                    if job.current_resolution_source() == ResolutionSource::QueueWarm {
                        if tier == PreloadTier::Immediate {
                            if take_over_queue_warm_with_demand_extract(
                                &pipeline,
                                &internal_tx,
                                transport,
                                &track_id,
                                &job,
                                trigger,
                                tier,
                                "immediate_preempts_queue_warm",
                            ) {
                                continue;
                            }
                        }

                        match tokio::time::timeout(
                            SHARED_JOB_JOIN_TIMEOUT,
                            job.wait_for_update(JobProgress::ResolvingUrl),
                        )
                        .await
                        {
                            Ok(()) => {}
                            Err(_) => {
                                take_over_queue_warm_with_demand_extract(
                                    &pipeline,
                                    &internal_tx,
                                    transport,
                                    &track_id,
                                    &job,
                                    trigger,
                                    tier,
                                    "queue_warm_join_timeout",
                                );
                            }
                        }
                    } else {
                        job.wait_for_update(JobProgress::ResolvingUrl).await;
                    }
                }
                JobProgress::Completed(result) => {
                    return result;
                }
                JobProgress::UrlResolved { track_id, stream_url } => {
                    match decide_url_resolved_action(
                        &pipeline,
                        allow_immediate_direct,
                        stream_immediate_cache_miss,
                        tier,
                        &track_id,
                        &stream_url,
                    ) {
                        UrlResolvedAction::Complete(result) => {
                            job.complete(result.clone());
                            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
                            return result;
                        }
                        UrlResolvedAction::WaitForPrefixDownload { track_id, stream_url } => {
                            let should_spawn = job.transition_from_url_resolved_to_downloading(
                                &track_id,
                                &stream_url,
                            );

                            if should_spawn {
                                Self::spawn_prefix_download(
                                    Arc::clone(&pipeline),
                                    permits.clone(),
                                    internal_tx.clone(),
                                    next_trace_id(),
                                    track_id.clone(),
                                    Arc::clone(&job),
                                    stream_url.clone(),
                                    tier,
                                );
                            }

                            return Self::wait_for_prefix_result(
                                job,
                                stream_url,
                                tier,
                                deadline,
                                allow_immediate_direct,
                            )
                            .await;
                        }
                    }
                }
                JobProgress::DownloadingPrefix { stream_url } => {
                    if let Some(result) =
                        direct_bypass_for_inflight_prefix(tier, allow_immediate_direct, &stream_url)
                    {
                        return result;
                    }

                    return Self::wait_for_prefix_result(
                        job,
                        stream_url,
                        tier,
                        deadline,
                        allow_immediate_direct,
                    )
                    .await;
                }
            }
        }
    }

    fn spawn_prefix_download(
        pipeline: Arc<StagingPipeline>,
        permits: TierPermits,
        internal_tx: mpsc::UnboundedSender<InternalEvent>,
        trace_token: u64,
        track_id: String,
        job: Arc<InFlightJob>,
        stream_url: String,
        tier: PreloadTier,
    ) {
        let task_job = Arc::clone(&job);
        let task_token = job.allocate_task_token();

        log::debug!(
            "[TRACE] prefix_start trace_id={} track_id={} tier={:?}",
            trace_token,
            track_id,
            tier,
        );

        let task = tokio::spawn(async move {
            let _permit = permits.acquire(tier).await;

            let outcome = match pipeline.ensure_prefix(&track_id, &stream_url).await {
                Ok((prefix_path, prefix_bytes, content_length)) => {
                    log::debug!(
                        "[TRACE] prefix_result trace_id={} track_id={} result=ok tier={:?} prefix_bytes={} content_length={}",
                        trace_token,
                        track_id,
                        tier,
                        prefix_bytes,
                        content_length,
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
                        "[TRACE] prefix_result trace_id={} track_id={} result=fail tier={:?} error={}",
                        trace_token,
                        track_id,
                        tier,
                        e,
                    );
                    PrepareResult::Failed(e.to_string())
                }
            };

            task_job.complete(outcome);
            task_job.clear_task(task_token);
            let _ = internal_tx.send(InternalEvent::JobFinished { track_id });
        });

        job.replace_task(task_token, task);
    }

    async fn wait_for_prefix_result(
        job: Arc<InFlightJob>,
        stream_url: String,
        tier: PreloadTier,
        deadline: Option<Duration>,
        allow_immediate_direct: bool,
    ) -> PrepareResult {
        let wait = job.wait_for_completion();

        if tier == PreloadTier::Immediate && allow_immediate_direct {
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
                    Ok(result @ PrepareResult::StreamAndCache { .. }) => result,
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
                    result @ PrepareResult::StreamAndCache { .. } => result,
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

    pub fn warm_url(&self, track_id: String) {
        if let Err(e) = self.tx.try_send(CacheRequest::Warm { track_id }) {
            log::debug!("YouTubeMediaPreparer warm dropped: {e}");
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

    pub fn apply_plan(&self, plan: MediaPreparationPlan) {
        if let Err(e) = self.tx.blocking_send(CacheRequest::ApplyPlan { plan }) {
            log::debug!("YouTubeMediaPreparer apply_plan dropped: {e}");
        }
    }

    pub fn shutdown(&self) {
        if let Err(e) = self.tx.try_send(CacheRequest::Shutdown) {
            log::debug!("YouTubeMediaPreparer shutdown dropped: {e}");
        }
    }

    pub fn warm_many_urls(&self, track_ids: Vec<String>) {
        if track_ids.is_empty() {
            return;
        }

        if let Err(e) = self.tx.try_send(CacheRequest::WarmBatch { track_ids }) {
            log::debug!("YouTubeMediaPreparer batch warm dropped: {e}");
        }
    }
}

fn log_in_memory_job_state(
    track_id: &str,
    trigger: &'static str,
    tier: Option<PreloadTier>,
    progress: &JobProgress,
) {
    log::debug!(
        track_id = track_id,
        trigger = trigger,
        tier:? = tier,
        state = job_progress_label(progress),
        source = "in_memory_job_state";
        "[TRACE] Reusing existing media job state"
    );
}

fn job_progress_label(progress: &JobProgress) -> &'static str {
    match progress {
        JobProgress::ResolvingUrl => "resolving_url",
        JobProgress::UrlResolved { .. } => "url_resolved",
        JobProgress::DownloadingPrefix { .. } => "downloading_prefix",
        JobProgress::Completed(PrepareResult::StagedPrefix { .. }) => "completed_staged_prefix",
        JobProgress::Completed(PrepareResult::StreamAndCache { .. }) => {
            "completed_stream_and_cache"
        }
        JobProgress::Completed(PrepareResult::Direct { .. }) => "completed_direct",
        JobProgress::Completed(PrepareResult::Cancelled) => "completed_cancelled",
        JobProgress::Completed(PrepareResult::Failed(_)) => "completed_failed",
    }
}

fn prepared_media_from_result(result: PrepareResult) -> Result<PreparedMedia> {
    match result {
        PrepareResult::StagedPrefix { prefix_path, prefix_bytes, stream_url, content_length } => {
            Ok(PreparedMedia::StagedPrefix {
                path: prefix_path,
                bytes: prefix_bytes,
                url: stream_url,
                content_length,
            })
        }
        PrepareResult::StreamAndCache { stream_url, content_length, prefix_path, prefix_size } => {
            Ok(PreparedMedia::StreamAndCache {
                url: stream_url,
                content_length,
                prefix_path,
                prefix_size,
            })
        }
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

    fn warm(&self, track_id: &str) {
        self.warm_url(track_id.to_string());
    }

    fn warm_many(&self, track_ids: &[String]) {
        YouTubeMediaPreparerHandle::warm_many_urls(self, track_ids.to_vec());
    }

    fn extract_state(&self, track_id: &str) -> ExtractRegistryState {
        self.job_registry.extract_state(track_id)
    }

    fn wait_for_extract_state_change(
        &self,
        track_id: &str,
        observed: ExtractRegistryState,
        timeout: Duration,
    ) -> ExtractRegistryState {
        self.job_registry.wait_for_extract_state_change(track_id, observed, timeout)
    }

    fn invalidate(&self, track_id: &str) {
        self.url_resolver.invalidate(track_id);
    }

    fn activate_playback_window(&self, track_ids: &[String]) {
        YouTubeMediaPreparerHandle::activate_playback_window(self, track_ids.to_vec());
    }

    fn apply_plan(&self, plan: MediaPreparationPlan) {
        YouTubeMediaPreparerHandle::apply_plan(self, plan);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::super::job_registry::{JobProgress, JobState};
    use super::{
        InFlightJob, JobRegistry, MAX_PENDING_PRELOADS, PreloadJob, PrepareResult, StagingPipeline,
        TierPermits, YouTubeMediaPreparer, prepared_media_from_result,
    };
    use crate::backends::youtube::{
        audio::{AudioCache, AudioDeliveryPlanner, CacheConfig},
        config::{AudioDeliveryMode, BackgroundExtractMode, ExtractorType},
        extractor::Extractor,
        media::{MediaPreparationPlan, PreloadTier, PreparedMedia},
        url_resolver::UrlResolver,
    };

    fn build_test_preparer_with_resolver(
        mode: AudioDeliveryMode,
        url_resolver: Arc<UrlResolver>,
    ) -> (TempDir, YouTubeMediaPreparer) {
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
        let audio_source_plan = AudioDeliveryPlanner.plan(mode);
        let pipeline = Arc::new(StagingPipeline::new(url_resolver, cache, audio_source_plan));

        let preparer = YouTubeMediaPreparer {
            rx,
            job_registry: Arc::new(JobRegistry::default()),
            request_to_track: Default::default(),
            active_window: Default::default(),
            last_applied_active_window: None,
            last_extract_scope: None,
            last_extract_scope_generation: None,
            last_prefix_targets: None,
            pipeline,
            audio_source_plan,
            background_queue: Default::default(),
            permits: TierPermits::new(),
            internal_rx,
            internal_tx,
        };

        drop(tx);
        (temp_dir, preparer)
    }

    fn build_test_preparer_for_mode(mode: AudioDeliveryMode) -> (TempDir, YouTubeMediaPreparer) {
        build_test_preparer_with_resolver(mode, Arc::new(UrlResolver::default()))
    }

    fn build_test_preparer() -> (TempDir, YouTubeMediaPreparer) {
        build_test_preparer_for_mode(AudioDeliveryMode::Staged)
    }

    fn spawn_prefix_probe_server() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test prefix probe server");
        let addr = listener.local_addr().expect("read test prefix probe addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_thread = Arc::clone(&hits);

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                hits_for_thread.fetch_add(1, Ordering::SeqCst);

                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf);
                let body = vec![0_u8; 1024];
                let response = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-1023/4096\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        (format!("http://{addr}/audio?clen=4096"), hits)
    }

    struct SequenceExtractor {
        base_url: String,
        extract_one_count: AtomicUsize,
    }

    impl SequenceExtractor {
        fn new(base_url: String) -> Self {
            Self { base_url, extract_one_count: AtomicUsize::new(0) }
        }
    }

    impl Extractor for SequenceExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, anyhow::Result<String>> {
            video_ids
                .iter()
                .map(|video_id| (video_id.clone(), self.extract_one(video_id)))
                .collect()
        }

        fn name(&self) -> &'static str {
            "sequence"
        }

        fn extract_one(&self, _video_id: &str) -> anyhow::Result<String> {
            let seq = self.extract_one_count.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(format!("{}&seq={seq}", self.base_url))
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    #[derive(Clone)]
    struct BatchAwareExtractor {
        batch_count: Arc<AtomicUsize>,
        one_count: Arc<AtomicUsize>,
    }

    impl BatchAwareExtractor {
        fn new() -> Self {
            Self {
                batch_count: Arc::new(AtomicUsize::new(0)),
                one_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn batch_count(&self) -> usize {
            self.batch_count.load(Ordering::SeqCst)
        }

        fn one_count(&self) -> usize {
            self.one_count.load(Ordering::SeqCst)
        }
    }

    impl Extractor for BatchAwareExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, anyhow::Result<String>> {
            self.batch_count.fetch_add(1, Ordering::SeqCst);
            video_ids
                .iter()
                .map(|video_id| (video_id.clone(), Ok(format!("https://example.com/{video_id}"))))
                .collect()
        }

        fn name(&self) -> &'static str {
            "batch-aware"
        }

        fn extract_one(&self, video_id: &str) -> anyhow::Result<String> {
            self.one_count.fetch_add(1, Ordering::SeqCst);
            Ok(format!("https://example.com/{video_id}"))
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    #[derive(Clone)]
    struct SlowBatchExtractor {
        batch_count: Arc<AtomicUsize>,
        one_count: Arc<AtomicUsize>,
        batch_delay: Duration,
    }

    impl SlowBatchExtractor {
        fn new(batch_delay: Duration) -> Self {
            Self {
                batch_count: Arc::new(AtomicUsize::new(0)),
                one_count: Arc::new(AtomicUsize::new(0)),
                batch_delay,
            }
        }

        fn batch_count(&self) -> usize {
            self.batch_count.load(Ordering::SeqCst)
        }

        fn one_count(&self) -> usize {
            self.one_count.load(Ordering::SeqCst)
        }
    }

    impl Extractor for SlowBatchExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, anyhow::Result<String>> {
            self.batch_count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.batch_delay);
            video_ids
                .iter()
                .map(|video_id| {
                    (video_id.clone(), Ok(format!("https://example.com/batch/{video_id}")))
                })
                .collect()
        }

        fn name(&self) -> &'static str {
            "slow-batch"
        }

        fn extract_one(&self, video_id: &str) -> anyhow::Result<String> {
            self.one_count.fetch_add(1, Ordering::SeqCst);
            Ok(format!("https://example.com/single/{video_id}"))
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
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
    async fn immediate_failed_prepare_falls_back_to_direct() {
        let job = Arc::new(InFlightJob::new("track-1".to_string()));
        job.set_state(JobState::Completed(PrepareResult::Failed("boom".to_string())));

        let result = YouTubeMediaPreparer::wait_for_prefix_result(
            job,
            "https://example.com/direct".to_string(),
            PreloadTier::Immediate,
            Some(Duration::from_millis(10)),
            true,
        )
        .await;

        assert!(matches!(
            result,
            PrepareResult::Direct {
                stream_url
            } if stream_url == "https://example.com/direct"
        ));
    }

    #[tokio::test]
    async fn immediate_cache_miss_streams_and_caches_in_auto_mode() {
        let (_temp_dir, preparer) = build_test_preparer_for_mode(AudioDeliveryMode::Auto);
        let (stream_url, hits) = spawn_prefix_probe_server();
        let track_id = "immediate-track";

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        job.set_state(JobState::UrlResolved {
            track_id: track_id.to_string(),
            stream_url: stream_url.clone(),
        });

        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let result = YouTubeMediaPreparer::wait_or_coalesce_impl(
            Arc::clone(&preparer.pipeline),
            preparer.permits.clone(),
            internal_tx,
            preparer.audio_source_plan.allows_immediate_direct(),
            preparer.audio_source_plan.streams_immediate_cache_miss_via_relay(),
            preparer.audio_source_plan.transport,
            track_id.to_string(),
            Arc::clone(&job),
            "test",
            PreloadTier::Immediate,
            Some(Duration::from_millis(10)),
        )
        .await;

        assert!(matches!(
            result,
            PrepareResult::StreamAndCache {
                stream_url: returned_url,
                content_length,
                prefix_path,
                prefix_size,
            } if returned_url == stream_url
                && content_length == 4096
                && prefix_path == preparer.pipeline.prefix_path_for(track_id)
                && prefix_size == preparer.pipeline.default_prefix_size()
        ));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn immediate_cache_miss_streams_and_caches_in_relay_mode() {
        let (_temp_dir, preparer) = build_test_preparer_for_mode(AudioDeliveryMode::Relay);
        let (stream_url, hits) = spawn_prefix_probe_server();
        let track_id = "relay-track";

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        job.set_state(JobState::UrlResolved {
            track_id: track_id.to_string(),
            stream_url: stream_url.clone(),
        });

        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let result = YouTubeMediaPreparer::wait_or_coalesce_impl(
            Arc::clone(&preparer.pipeline),
            preparer.permits.clone(),
            internal_tx,
            preparer.audio_source_plan.allows_immediate_direct(),
            preparer.audio_source_plan.streams_immediate_cache_miss_via_relay(),
            preparer.audio_source_plan.transport,
            track_id.to_string(),
            Arc::clone(&job),
            "test",
            PreloadTier::Immediate,
            Some(Duration::from_secs(1)),
        )
        .await;

        assert!(matches!(
            result,
            PrepareResult::StreamAndCache {
                stream_url: returned_url,
                content_length,
                prefix_path,
                prefix_size,
            } if returned_url == stream_url
                && content_length == 4096
                && prefix_path == preparer.pipeline.prefix_path_for(track_id)
                && prefix_size == preparer.pipeline.default_prefix_size()
        ));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn relay_mode_gapless_cache_miss_reuses_original_stream_url_after_prefix_download() {
        let (base_url, _hits) = spawn_prefix_probe_server();
        let extractor = Arc::new(SequenceExtractor::new(base_url.clone()));
        let url_resolver = Arc::new(UrlResolver::from_extractor(
            extractor.clone(),
            crate::backends::youtube::config::ExtractorType::YtDlp,
        ));
        let (_temp_dir, preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Relay, url_resolver.clone());
        let track_id = "relay-refresh-track";
        let initial_url = url_resolver.get_url(track_id).unwrap();

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        job.set_state(JobState::UrlResolved {
            track_id: track_id.to_string(),
            stream_url: initial_url.clone(),
        });

        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let result = YouTubeMediaPreparer::wait_or_coalesce_impl(
            Arc::clone(&preparer.pipeline),
            preparer.permits.clone(),
            internal_tx,
            preparer.audio_source_plan.allows_immediate_direct(),
            preparer.audio_source_plan.streams_immediate_cache_miss_via_relay(),
            preparer.audio_source_plan.transport,
            track_id.to_string(),
            Arc::clone(&job),
            "test",
            PreloadTier::Gapless,
            Some(Duration::from_secs(1)),
        )
        .await;

        assert!(matches!(
            result,
            PrepareResult::StagedPrefix {
                stream_url: returned_url,
                ..
            } if returned_url == initial_url
        ));
        assert_eq!(extractor.extract_one_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_batch_uses_batch_extractor_path() {
        let extractor = BatchAwareExtractor::new();
        let cached =
            Arc::new(crate::backends::youtube::extractor::CachedExtractor::new(extractor.clone()));
        let resolver = Arc::new(UrlResolver::from_extractor(
            cached,
            crate::backends::youtube::config::ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Staged, resolver.clone());

        preparer.handle_warm_batch_request(vec!["track-a".to_string(), "track-b".to_string()]);

        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if extractor.batch_count() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("warm batch should invoke batch extractor");
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 0);
        assert!(resolver.get_url("track-a").is_ok());
        assert!(resolver.get_url("track-b").is_ok());
        assert_eq!(extractor.one_count(), 0);
    }

    #[tokio::test]
    async fn warm_batch_only_sends_uncached_tracks_to_real_extractor() {
        let extractor = BatchAwareExtractor::new();
        let cached =
            Arc::new(crate::backends::youtube::extractor::CachedExtractor::new(extractor.clone()));
        let resolver = Arc::new(UrlResolver::from_extractor(
            cached,
            crate::backends::youtube::config::ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Staged, resolver.clone());

        assert!(resolver.get_url("track-a").is_ok());
        assert_eq!(extractor.batch_count(), 0);
        assert_eq!(extractor.one_count(), 1);

        preparer.handle_warm_batch_request(vec!["track-a".to_string(), "track-b".to_string()]);

        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if extractor.batch_count() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("warm batch should invoke batch extractor");
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 1);
        assert!(resolver.get_url("track-b").is_ok());
        assert_eq!(extractor.one_count(), 1);
    }

    #[tokio::test]
    async fn apply_plan_reconciles_active_window_and_extract_scope_once() {
        let extractor = BatchAwareExtractor::new();
        let cached =
            Arc::new(crate::backends::youtube::extractor::CachedExtractor::new(extractor.clone()));
        let resolver = Arc::new(UrlResolver::from_extractor(cached, ExtractorType::Ytx));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Staged, resolver);

        let plan = MediaPreparationPlan {
            background_extract_mode: BackgroundExtractMode::Balanced,
            active_window: vec!["current".to_string(), "next".to_string()],
            prefix_targets: vec!["next".to_string()],
            extract_scope_generation: 1,
            extract_scope: vec!["next".to_string(), "later".to_string()],
        };

        preparer.handle_apply_plan(plan.clone());
        preparer.handle_apply_plan(plan);

        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if extractor.batch_count() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("apply_plan should batch-warm extract scope once");
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(
            preparer.last_applied_active_window,
            Some(vec!["current".to_string(), "next".to_string()])
        );
        assert!(preparer.active_window.contains("current"));
        assert!(preparer.active_window.contains("next"));
        assert_eq!(preparer.last_prefix_targets, Some(vec!["next".to_string()]));
        assert_eq!(preparer.background_queue.len(), 1);
        assert_eq!(
            preparer.background_queue.front().map(|job| job.track_id.as_str()),
            Some("next")
        );
        assert_eq!(
            preparer.last_extract_scope,
            Some(vec!["next".to_string(), "later".to_string()])
        );
        assert_eq!(preparer.last_extract_scope_generation, Some(1));
    }

    #[tokio::test]
    async fn prepare_replaces_stalled_queue_warm_with_extract_one_and_drops_late_batch_result() {
        let extractor = Arc::new(SlowBatchExtractor::new(Duration::from_millis(150)));
        let resolver = Arc::new(UrlResolver::from_cached_extractor(
            extractor.as_ref().clone(),
            ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Direct, resolver);
        let track_id = "queue-warm-timeout";

        preparer.handle_warm_batch_request(vec![track_id.to_string()]);

        let result = preparer
            .prepare(track_id.to_string(), PreloadTier::Immediate, Some(Duration::from_millis(5)))
            .await;

        assert!(matches!(
            result,
            PrepareResult::Direct { ref stream_url }
                if stream_url == "https://example.com/single/queue-warm-timeout"
        ));
        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 1);

        tokio::time::sleep(Duration::from_millis(175)).await;
        let job = preparer.job_registry.get(track_id).expect("job should remain registered");
        assert!(matches!(
            job.snapshot(),
            JobState::Completed(PrepareResult::Direct { ref stream_url })
                if stream_url == "https://example.com/single/queue-warm-timeout"
        ));
    }

    #[tokio::test]
    async fn immediate_prepare_does_not_wait_for_queue_warm_join_timeout() {
        let extractor = Arc::new(SlowBatchExtractor::new(Duration::from_millis(150)));
        let resolver = Arc::new(UrlResolver::from_cached_extractor(
            extractor.as_ref().clone(),
            ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Direct, resolver);
        let track_id = "queue-warm-immediate-preempt";

        preparer.handle_warm_batch_request(vec![track_id.to_string()]);

        let result = tokio::time::timeout(
            Duration::from_millis(45),
            preparer.prepare(
                track_id.to_string(),
                PreloadTier::Immediate,
                Some(Duration::from_secs(1)),
            ),
        )
        .await
        .expect("immediate prepare should not wait for queue-warm join timeout");

        assert!(matches!(
            result,
            PrepareResult::Direct { ref stream_url }
                if stream_url == "https://example.com/single/queue-warm-immediate-preempt"
        ));
        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 1);

        // Verify late batch result is dropped and demand result wins
        tokio::time::sleep(Duration::from_millis(175)).await;
        let job = preparer.job_registry.get(track_id).expect("job should remain registered");
        assert!(matches!(
            job.snapshot(),
            JobState::Completed(PrepareResult::Direct { ref stream_url })
                if stream_url == "https://example.com/single/queue-warm-immediate-preempt"
        ));
    }

    #[tokio::test]
    async fn prefix_preload_replaces_stalled_queue_warm_with_extract_one() {
        let extractor = Arc::new(SlowBatchExtractor::new(Duration::from_millis(150)));
        let resolver = Arc::new(UrlResolver::from_cached_extractor(
            extractor.as_ref().clone(),
            ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Direct, resolver);
        let track_id = "prefix-timeout";

        preparer.handle_warm_batch_request(vec![track_id.to_string()]);
        preparer.handle_preload_request(track_id.to_string(), PreloadTier::Background, 77);
        preparer.dispatch_background_job();

        let job = preparer.job_registry.get(track_id).expect("job should be registered");
        let result = tokio::time::timeout(Duration::from_millis(300), job.wait_for_completion())
            .await
            .expect("prefix preload should complete after timeout replacement");

        assert!(matches!(
            result,
            PrepareResult::Direct { ref stream_url }
                if stream_url == "https://example.com/single/prefix-timeout"
        ));
        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 1);

        tokio::time::sleep(Duration::from_millis(175)).await;
        assert!(matches!(
            job.snapshot(),
            JobState::Completed(PrepareResult::Direct { ref stream_url })
                if stream_url == "https://example.com/single/prefix-timeout"
        ));
    }

    #[tokio::test]
    async fn cache_hit_returns_staged_prefix_without_download() {
        let (temp_dir, preparer) = build_test_preparer();
        let track_id = "cached-track";
        let stream_url = "https://example.com/stream".to_string();
        let prefix_path = temp_dir.path().join("cached-track.webm");
        std::fs::write(&prefix_path, vec![0u8; 512]).unwrap();
        preparer.pipeline.audio_cache().register_prefix(track_id, prefix_path.clone(), 512, 4096);

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        job.set_state(JobState::UrlResolved {
            track_id: track_id.to_string(),
            stream_url: stream_url.clone(),
        });
        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();

        let result = YouTubeMediaPreparer::wait_or_coalesce_impl(
            Arc::clone(&preparer.pipeline),
            preparer.permits.clone(),
            internal_tx,
            preparer.audio_source_plan.allows_immediate_direct(),
            preparer.audio_source_plan.streams_immediate_cache_miss_via_relay(),
            preparer.audio_source_plan.transport,
            track_id.to_string(),
            job,
            "test",
            PreloadTier::Gapless,
            None,
        )
        .await;

        assert!(matches!(
            result,
            PrepareResult::StagedPrefix {
                prefix_path: returned_path,
                prefix_bytes,
                stream_url: returned_url,
                content_length,
            } if returned_path == prefix_path
                && prefix_bytes == 512
                && returned_url == stream_url
                && content_length == 4096
        ));
    }

    #[test]
    fn direct_mode_plan_disables_local_staging() {
        let (_temp_dir, preparer) = build_test_preparer_for_mode(AudioDeliveryMode::Direct);
        assert!(!preparer.audio_source_plan.uses_local_staging());
    }

    #[tokio::test]
    async fn activating_new_window_keeps_inflight_extracts_but_drops_stale_preloads() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let stale_job = Arc::new(InFlightJob::new("stale".to_string()));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        let stale_task_token = stale_job.allocate_task_token();
        stale_job.replace_task(
            stale_task_token,
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let _ = done_tx.send(());
            }),
        );

        preparer.job_registry.insert("stale".to_string(), Arc::clone(&stale_job));
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

        assert!(preparer.job_registry.contains("stale"));
        assert!(matches!(stale_job.snapshot(), JobState::ResolvingUrl { .. }));
        assert_eq!(preparer.background_queue.len(), 1);
        assert_eq!(preparer.background_queue[0].track_id, "keep");
        assert!(!preparer.request_to_track.contains_key(&1));
        assert!(preparer.request_to_track.contains_key(&2));
        assert!(matches!(tokio::time::timeout(Duration::from_millis(50), done_rx).await, Err(_)));

        stale_job.cancel();
    }

    #[tokio::test]
    async fn cancel_request_aborts_inflight_preload() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let stale_job = Arc::new(InFlightJob::new("stale".to_string()));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        let stale_task_token = stale_job.allocate_task_token();
        stale_job.replace_task(
            stale_task_token,
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let _ = done_tx.send(());
            }),
        );

        preparer.job_registry.insert("stale".to_string(), Arc::clone(&stale_job));
        preparer.request_to_track.insert(41, "stale".to_string());

        preparer.handle_cancel_request(41);

        assert!(!preparer.job_registry.contains("stale"));
        assert!(matches!(stale_job.snapshot(), JobState::Completed(PrepareResult::Cancelled)));
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

        let shared_task_token = shared_job.allocate_task_token();
        shared_job.replace_task(
            shared_task_token,
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let _ = done_tx.send(());
            }),
        );

        preparer.job_registry.insert("shared".to_string(), Arc::clone(&shared_job));
        preparer.request_to_track.insert(41, "shared".to_string());
        preparer.request_to_track.insert(42, "shared".to_string());

        preparer.handle_cancel_request(41);

        assert!(preparer.job_registry.contains("shared"));
        assert!(!preparer.request_to_track.contains_key(&41));
        assert!(preparer.request_to_track.contains_key(&42));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(50), &mut Box::pin(done_rx)).await,
            Err(_)
        ));

        preparer.handle_cancel_request(42);

        assert!(!preparer.job_registry.contains("shared"));
        assert!(!preparer.request_to_track.contains_key(&42));
        assert!(matches!(shared_job.snapshot(), JobState::Completed(PrepareResult::Cancelled)));
    }

    #[tokio::test]
    async fn cancel_request_keeps_shared_track_alive_when_queue_warm_intent_exists() {
        let (_temp_dir, mut preparer) = build_test_preparer();
        let shared_job = Arc::new(InFlightJob::new("shared".to_string()));

        preparer.job_registry.insert("shared".to_string(), Arc::clone(&shared_job));
        preparer.request_to_track.insert(41, "shared".to_string());
        preparer.last_extract_scope = Some(vec!["shared".to_string()]);

        preparer.handle_cancel_request(41);

        assert!(preparer.job_registry.contains("shared"));
        assert!(matches!(shared_job.snapshot(), JobState::ResolvingUrl { .. }));
        assert!(!preparer.request_to_track.contains_key(&41));
    }

    #[tokio::test]
    async fn warm_job_can_be_promoted_to_staged_prefetch() {
        let (temp_dir, mut preparer) = build_test_preparer();
        let track_id = "queued-track";
        let prefix_path = temp_dir.path().join("queued-track.webm");
        std::fs::write(&prefix_path, vec![0u8; 256]).unwrap();
        preparer.pipeline.audio_cache().register_prefix(track_id, prefix_path.clone(), 256, 4096);

        let warm_job = Arc::new(InFlightJob::new(track_id.to_string()));
        warm_job.set_state(JobState::UrlResolved {
            track_id: track_id.to_string(),
            stream_url: "https://example.com/stream".to_string(),
        });
        preparer.job_registry.insert(track_id.to_string(), Arc::clone(&warm_job));

        preparer.handle_preload_request(track_id.to_string(), PreloadTier::Gapless, 7);

        assert_eq!(preparer.background_queue.len(), 1);
        assert_eq!(preparer.background_queue[0].track_id, track_id);

        preparer.dispatch_background_job();

        let result = tokio::time::timeout(Duration::from_secs(1), warm_job.wait_for_completion())
            .await
            .expect("warm job should complete after promotion");

        assert!(matches!(
            result,
            PrepareResult::StagedPrefix {
                prefix_path: returned_path,
                prefix_bytes,
                content_length,
                ..
            } if returned_path == prefix_path && prefix_bytes == 256 && content_length == 4096
        ));
    }

    #[tokio::test]
    async fn cancelled_queue_warm_job_is_not_resurrected_by_late_batch_result() {
        let extractor = Arc::new(SlowBatchExtractor::new(Duration::from_millis(150)));
        let resolver = Arc::new(UrlResolver::from_cached_extractor(
            extractor.as_ref().clone(),
            ExtractorType::Ytx,
        ));
        let (_temp_dir, mut preparer) =
            build_test_preparer_with_resolver(AudioDeliveryMode::Direct, resolver);
        let track_id = "cancelled-batch";

        preparer.handle_warm_batch_request(vec![track_id.to_string()]);
        let job = preparer.job_registry.get(track_id).expect("job should be registered");

        job.cancel();
        tokio::time::sleep(Duration::from_millis(175)).await;

        assert!(matches!(job.snapshot(), JobState::Completed(PrepareResult::Cancelled)));
        assert_eq!(extractor.batch_count(), 1);
        assert_eq!(extractor.one_count(), 0);
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

        let queued: Vec<String> =
            preparer.background_queue.iter().map(|job| job.track_id.clone()).collect();
        let expected: Vec<String> =
            (3..(MAX_PENDING_PRELOADS + 3)).map(|idx| format!("track-{idx}")).collect();

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

        preparer.handle_preload_request("gapless-priority".to_string(), PreloadTier::Gapless, 99);
        preparer.handle_preload_request(
            "immediate-priority".to_string(),
            PreloadTier::Immediate,
            100,
        );

        let queued: Vec<(String, PreloadTier)> =
            preparer.background_queue.iter().map(|job| (job.track_id.clone(), job.tier)).collect();

        assert_eq!(preparer.background_queue.len(), MAX_PENDING_PRELOADS);
        assert!(queued.contains(&("gapless-priority".to_string(), PreloadTier::Gapless)));
        assert!(queued.contains(&("immediate-priority".to_string(), PreloadTier::Immediate)));
        assert!(!preparer.request_to_track.contains_key(&1));
        assert!(!preparer.request_to_track.contains_key(&2));
        assert!(preparer.request_to_track.contains_key(&99));
        assert!(preparer.request_to_track.contains_key(&100));
    }
}
