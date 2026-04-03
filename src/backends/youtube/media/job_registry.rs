use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use parking_lot::{Condvar, Mutex};
use tokio::{sync::watch, task::JoinHandle};

use super::preparer::PrepareResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractRegistryState {
    Absent,
    Running,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionSource {
    QueueWarm,
    Demand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolutionLease {
    token: u64,
    source: ResolutionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionApplyOutcome {
    Applied { completed: bool },
    DroppedStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JobProgress {
    ResolvingUrl,
    UrlResolved { track_id: String, stream_url: String },
    DownloadingPrefix { stream_url: String },
    Completed(PrepareResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JobState {
    ResolvingUrl { track_id: String },
    UrlResolved { track_id: String, stream_url: String },
    DownloadingPrefix { track_id: String, stream_url: String },
    Completed(PrepareResult),
}

#[derive(Debug)]
pub(super) struct InFlightJob {
    state: Mutex<JobState>,
    resolution_lease: Mutex<ResolutionLease>,
    task: Mutex<Option<(u64, JoinHandle<()>)>>,
    next_resolution_token: AtomicU64,
    next_task_token: AtomicU64,
    signal: watch::Sender<JobState>,
    changed: Condvar,
}

impl InFlightJob {
    pub(super) fn new(track_id: String) -> Self {
        let initial_state = JobState::ResolvingUrl { track_id };
        let (signal, _rx) = watch::channel(initial_state.clone());
        Self {
            state: Mutex::new(initial_state),
            resolution_lease: Mutex::new(ResolutionLease {
                token: 0,
                source: ResolutionSource::QueueWarm,
            }),
            task: Mutex::new(None),
            next_resolution_token: AtomicU64::new(1),
            next_task_token: AtomicU64::new(1),
            signal,
            changed: Condvar::new(),
        }
    }

    pub(super) fn snapshot(&self) -> JobState {
        self.state.lock().clone()
    }

    pub(super) fn set_state(&self, new_state: JobState) {
        *self.state.lock() = new_state.clone();
        self.signal.send_replace(new_state);
        self.changed.notify_all();
    }

    pub(super) fn progress(&self) -> JobProgress {
        Self::progress_from_state(&self.snapshot())
    }

    pub(super) fn extract_state(&self) -> ExtractRegistryState {
        Self::extract_state_from_state(&self.snapshot())
    }

    fn progress_from_state(state: &JobState) -> JobProgress {
        match state.clone() {
            JobState::ResolvingUrl { .. } => JobProgress::ResolvingUrl,
            JobState::UrlResolved { track_id, stream_url } => {
                JobProgress::UrlResolved { track_id, stream_url }
            }
            JobState::DownloadingPrefix { stream_url, .. } => {
                JobProgress::DownloadingPrefix { stream_url }
            }
            JobState::Completed(result) => JobProgress::Completed(result),
        }
    }

    fn extract_state_from_state(state: &JobState) -> ExtractRegistryState {
        match state {
            JobState::ResolvingUrl { .. } => ExtractRegistryState::Running,
            JobState::UrlResolved { .. } | JobState::DownloadingPrefix { .. } => {
                ExtractRegistryState::Ready
            }
            JobState::Completed(PrepareResult::Cancelled | PrepareResult::Failed(_)) => {
                ExtractRegistryState::Failed
            }
            JobState::Completed(_) => ExtractRegistryState::Ready,
        }
    }

    pub(super) fn claim_resolution_lease(&self, source: ResolutionSource) -> u64 {
        let token = self.next_resolution_token.fetch_add(1, Ordering::Relaxed);
        *self.resolution_lease.lock() = ResolutionLease { token, source };
        token
    }

    pub(super) fn current_resolution_source(&self) -> ResolutionSource {
        self.resolution_lease.lock().source
    }

    pub(super) fn replace_queue_warm_lease_with_demand(&self) -> Option<u64> {
        let mut lease = self.resolution_lease.lock();
        if lease.source != ResolutionSource::QueueWarm {
            return None;
        }

        if !matches!(*self.state.lock(), JobState::ResolvingUrl { .. }) {
            return None;
        }

        let token = self.next_resolution_token.fetch_add(1, Ordering::Relaxed);
        *lease = ResolutionLease { token, source: ResolutionSource::Demand };
        Some(token)
    }

    pub(super) fn apply_url_resolution_if_current(
        &self,
        resolution_token: u64,
        track_id: String,
        stream_url: String,
        uses_local_staging: bool,
    ) -> ResolutionApplyOutcome {
        if self.resolution_lease.lock().token != resolution_token {
            return ResolutionApplyOutcome::DroppedStale;
        }

        if !matches!(*self.state.lock(), JobState::ResolvingUrl { .. }) {
            return ResolutionApplyOutcome::DroppedStale;
        }

        if uses_local_staging {
            self.set_state(JobState::UrlResolved { track_id, stream_url });
            ResolutionApplyOutcome::Applied { completed: false }
        } else {
            self.complete(PrepareResult::Direct { stream_url });
            ResolutionApplyOutcome::Applied { completed: true }
        }
    }

    pub(super) fn complete(&self, result: PrepareResult) {
        self.set_state(JobState::Completed(result));
    }

    pub(super) fn fail(&self, error: impl Into<String>) {
        self.complete(PrepareResult::Failed(error.into()));
    }

    pub(super) fn fail_if_current(&self, resolution_token: u64, error: impl Into<String>) -> bool {
        if self.resolution_lease.lock().token != resolution_token {
            return false;
        }

        if !matches!(*self.state.lock(), JobState::ResolvingUrl { .. }) {
            return false;
        }

        self.fail(error);
        true
    }

    pub(super) fn transition_from_url_resolved_to_downloading(
        &self,
        track_id: &str,
        stream_url: &str,
    ) -> bool {
        let mut state = self.state.lock();
        match &*state {
            JobState::UrlResolved { track_id: current_id, stream_url: current }
                if current_id == track_id && current == stream_url =>
            {
                let new_state = JobState::DownloadingPrefix {
                    track_id: track_id.to_string(),
                    stream_url: stream_url.to_string(),
                };
                *state = new_state.clone();
                self.signal.send_replace(new_state);
                true
            }
            _ => false,
        }
    }

    pub(super) fn allocate_task_token(&self) -> u64 {
        self.next_task_token.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn replace_task(&self, task_token: u64, task: JoinHandle<()>) {
        if let Some((_, existing)) = self.task.lock().replace((task_token, task)) {
            existing.abort();
        }
    }

    pub(super) fn clear_task(&self, task_token: u64) {
        let mut task = self.task.lock();
        if task.as_ref().map(|(current_token, _)| *current_token) == Some(task_token) {
            task.take();
        }
    }

    pub(super) fn cancel(&self) {
        if let Some((_, task)) = self.task.lock().take() {
            task.abort();
        }

        if !matches!(self.snapshot(), JobState::Completed(_)) {
            self.complete(PrepareResult::Cancelled);
        }
    }

    pub(super) async fn wait_for_update(&self, observed: JobProgress) {
        let mut signal = self.signal.subscribe();

        if Self::progress_from_state(&signal.borrow().clone()) != observed {
            return;
        }

        loop {
            if signal.changed().await.is_err() {
                return;
            }

            if Self::progress_from_state(&signal.borrow().clone()) != observed {
                return;
            }
        }
    }

    pub(super) async fn wait_for_completion(&self) -> PrepareResult {
        let mut signal = self.signal.subscribe();

        loop {
            let state = signal.borrow().clone();

            match state {
                JobState::Completed(result) => return result,
                _ => {
                    if signal.changed().await.is_err() {
                        return match self.progress() {
                            JobProgress::Completed(result) => result,
                            _ => PrepareResult::Cancelled,
                        };
                    }
                }
            }
        }
    }

    pub(super) fn wait_for_extract_state_change(
        &self,
        observed: ExtractRegistryState,
        timeout: Duration,
    ) -> ExtractRegistryState {
        let mut state = self.state.lock();
        if Self::extract_state_from_state(&state) != observed {
            return Self::extract_state_from_state(&state);
        }

        let wait_result = self.changed.wait_for(&mut state, timeout);
        if wait_result.timed_out() && Self::extract_state_from_state(&state) == observed {
            return observed;
        }

        Self::extract_state_from_state(&state)
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{Duration, timeout};

    use super::*;

    #[tokio::test]
    async fn wait_for_update_observes_completion_that_happens_before_first_poll() {
        let job = InFlightJob::new("track-1".to_string());
        let wait = job.wait_for_update(JobProgress::ResolvingUrl);

        job.complete(PrepareResult::Direct {
            stream_url: "https://example.com/direct".to_string(),
        });

        timeout(Duration::from_millis(50), wait)
            .await
            .expect("wait_for_update should not miss completed state");
    }
}

#[derive(Debug, Default)]
pub(super) struct JobRegistry {
    in_flight: Mutex<HashMap<String, Arc<InFlightJob>>>,
}

impl JobRegistry {
    pub(super) fn get_or_create(&self, track_id: &str) -> (Arc<InFlightJob>, bool) {
        let mut in_flight = self.in_flight.lock();
        if let Some(existing) = in_flight.get(track_id) {
            return (Arc::clone(existing), false);
        }

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        in_flight.insert(track_id.to_string(), Arc::clone(&job));
        (job, true)
    }

    pub(super) fn get(&self, track_id: &str) -> Option<Arc<InFlightJob>> {
        self.in_flight.lock().get(track_id).map(Arc::clone)
    }

    pub(super) fn contains(&self, track_id: &str) -> bool {
        self.in_flight.lock().contains_key(track_id)
    }

    pub(super) fn progress(&self, track_id: &str) -> Option<JobProgress> {
        self.get(track_id).map(|job| job.progress())
    }

    pub(super) fn extract_state(&self, track_id: &str) -> ExtractRegistryState {
        self.get(track_id).map(|job| job.extract_state()).unwrap_or(ExtractRegistryState::Absent)
    }

    pub(super) fn wait_for_extract_state_change(
        &self,
        track_id: &str,
        observed: ExtractRegistryState,
        timeout: Duration,
    ) -> ExtractRegistryState {
        self.get(track_id)
            .map(|job| job.wait_for_extract_state_change(observed, timeout))
            .unwrap_or(ExtractRegistryState::Absent)
    }

    pub(super) fn remove(&self, track_id: &str) {
        self.in_flight.lock().remove(track_id);
    }

    pub(super) fn cancel_track(&self, track_id: &str) {
        if let Some(job) = self.in_flight.lock().remove(track_id) {
            job.cancel();
        }
    }

    #[cfg(test)]
    pub(super) fn insert(&self, track_id: String, job: Arc<InFlightJob>) {
        self.in_flight.lock().insert(track_id, job);
    }
}
