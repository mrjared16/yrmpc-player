use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinHandle};

use super::preparer::PrepareResult;

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
    task: Mutex<Option<JoinHandle<()>>>,
    signal: watch::Sender<JobState>,
}

impl InFlightJob {
    pub(super) fn new(track_id: String) -> Self {
        let initial_state = JobState::ResolvingUrl { track_id };
        let (signal, _rx) = watch::channel(initial_state.clone());
        Self { state: Mutex::new(initial_state), task: Mutex::new(None), signal }
    }

    pub(super) fn snapshot(&self) -> JobState {
        self.state.lock().clone()
    }

    pub(super) fn set_state(&self, new_state: JobState) {
        *self.state.lock() = new_state.clone();
        self.signal.send_replace(new_state);
    }

    pub(super) fn progress(&self) -> JobProgress {
        Self::progress_from_state(&self.snapshot())
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

    pub(super) fn apply_url_resolution(
        &self,
        track_id: String,
        stream_url: String,
        uses_local_staging: bool,
    ) -> bool {
        if uses_local_staging {
            self.set_state(JobState::UrlResolved { track_id, stream_url });
            false
        } else {
            self.complete(PrepareResult::Direct { stream_url });
            true
        }
    }

    pub(super) fn complete(&self, result: PrepareResult) {
        self.set_state(JobState::Completed(result));
    }

    pub(super) fn fail(&self, error: impl Into<String>) {
        self.complete(PrepareResult::Failed(error.into()));
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

    pub(super) fn replace_task(&self, task: JoinHandle<()>) {
        if let Some(existing) = self.task.lock().replace(task) {
            existing.abort();
        }
    }

    pub(super) fn clear_task(&self) {
        self.task.lock().take();
    }

    pub(super) fn cancel(&self) {
        if let Some(task) = self.task.lock().take() {
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
    in_flight: HashMap<String, Arc<InFlightJob>>,
}

impl JobRegistry {
    pub(super) fn get_or_create(&mut self, track_id: &str) -> (Arc<InFlightJob>, bool) {
        if let Some(existing) = self.in_flight.get(track_id) {
            return (Arc::clone(existing), false);
        }

        let job = Arc::new(InFlightJob::new(track_id.to_string()));
        self.in_flight.insert(track_id.to_string(), Arc::clone(&job));
        (job, true)
    }

    pub(super) fn contains(&self, track_id: &str) -> bool {
        self.in_flight.contains_key(track_id)
    }

    pub(super) fn progress(&self, track_id: &str) -> Option<JobProgress> {
        self.in_flight.get(track_id).map(|job| job.progress())
    }

    pub(super) fn remove(&mut self, track_id: &str) {
        self.in_flight.remove(track_id);
    }

    pub(super) fn cancel_track(&mut self, track_id: &str) {
        if let Some(job) = self.in_flight.remove(track_id) {
            job.cancel();
        }
    }

    pub(super) fn tracks_outside_window(&self, active_window: &HashSet<String>) -> Vec<String> {
        self.in_flight
            .keys()
            .filter(|track_id| !active_window.contains(*track_id))
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn insert(&mut self, track_id: String, job: Arc<InFlightJob>) {
        self.in_flight.insert(track_id, job);
    }
}
