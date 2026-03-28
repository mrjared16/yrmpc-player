use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use tokio::sync::Semaphore;

use super::super::protocol::play_intent::{PreloadTier, RequestId};

pub type TrackId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    StreamUrl,
    AudioPrefix,
}

#[derive(Debug)]
pub struct PreloadRequest {
    pub request_id: RequestId,
    pub track_id: TrackId,
    pub artifact: ArtifactKind,
    pub tier: PreloadTier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JobStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
}

#[derive(Debug)]
struct PreloadJob {
    request_id: RequestId,
    track_id: TrackId,
    artifact: ArtifactKind,
    tier: PreloadTier,
    status: JobStatus,
}

type JobKey = (TrackId, ArtifactKind);

#[derive(Debug, Default)]
struct Lane {
    queue: VecDeque<JobKey>,
    queued: HashSet<JobKey>,
}

impl Lane {
    fn push(&mut self, key: JobKey) {
        if self.queued.insert(key.clone()) {
            self.queue.push_back(key);
        }
    }

    fn remove(&mut self, key: &JobKey) {
        if self.queued.remove(key) {
            self.queue.retain(|existing| existing != key);
        }
    }

    #[cfg(test)]
    fn contains(&self, key: &JobKey) -> bool {
        self.queued.contains(key)
    }
}

#[derive(Debug)]
pub struct PreloadScheduler {
    jobs: HashMap<JobKey, PreloadJob>,

    immediate_lane: Lane,
    gapless_lane: Lane,
    eager_lane: Lane,
    background_lane: Lane,

    immediate_permits: Arc<Semaphore>,
    gapless_permits: Arc<Semaphore>,
    eager_permits: Arc<Semaphore>,
    background_permits: Arc<Semaphore>,
}

impl Default for PreloadScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PreloadScheduler {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            immediate_lane: Lane::default(),
            gapless_lane: Lane::default(),
            eager_lane: Lane::default(),
            background_lane: Lane::default(),
            immediate_permits: Arc::new(Semaphore::new(100)),
            gapless_permits: Arc::new(Semaphore::new(2)),
            eager_permits: Arc::new(Semaphore::new(1)),
            background_permits: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn submit(&mut self, request: PreloadRequest) {
        let key: JobKey = (request.track_id.clone(), request.artifact);

        let mut should_enqueue = false;
        let mut new_tier = request.tier;

        match self.jobs.get_mut(&key) {
            None => {
                self.jobs.insert(
                    key.clone(),
                    PreloadJob {
                        request_id: request.request_id,
                        track_id: request.track_id,
                        artifact: request.artifact,
                        tier: request.tier,
                        status: JobStatus::Pending,
                    },
                );
                should_enqueue = true;
            }
            Some(job) => {
                job.request_id = request.request_id;

                if request.tier > job.tier {
                    new_tier = request.tier;
                    job.tier = request.tier;
                    if matches!(job.status, JobStatus::Pending) {
                        should_enqueue = true;
                    }
                } else {
                    new_tier = job.tier;
                }
            }
        }

        if should_enqueue {
            self.remove_from_all_lanes(&key);
            self.enqueue_in_lane(new_tier, key.clone());
        }

        log::debug!("Submitted preload job: {} {:?} tier={:?}", key.0, key.1, new_tier);
    }

    pub fn cancel_request(&mut self, request_id: RequestId) {
        let keys_to_remove: Vec<JobKey> = self
            .jobs
            .iter()
            .filter_map(|(key, job)| {
                if job.request_id == request_id && matches!(job.status, JobStatus::Pending) {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();

        for key in &keys_to_remove {
            self.remove_from_all_lanes(key);
            self.jobs.remove(key);
        }

        log::debug!("Cancelled {} jobs for request {}", keys_to_remove.len(), request_id);
    }

    fn enqueue_in_lane(&mut self, tier: PreloadTier, key: JobKey) {
        match tier {
            PreloadTier::Immediate => self.immediate_lane.push(key),
            PreloadTier::Gapless => self.gapless_lane.push(key),
            PreloadTier::Eager => self.eager_lane.push(key),
            PreloadTier::Background => self.background_lane.push(key),
        }
    }

    fn remove_from_all_lanes(&mut self, key: &JobKey) {
        self.immediate_lane.remove(key);
        self.gapless_lane.remove(key);
        self.eager_lane.remove(key);
        self.background_lane.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        request_id: RequestId,
        track_id: &str,
        artifact: ArtifactKind,
        tier: PreloadTier,
    ) -> PreloadRequest {
        PreloadRequest { request_id, track_id: track_id.to_string(), artifact, tier }
    }

    #[test]
    fn test_submit_creates_job() {
        let mut scheduler = PreloadScheduler::new();
        scheduler.submit(request(1, "vid1", ArtifactKind::StreamUrl, PreloadTier::Eager));

        assert_eq!(scheduler.jobs.len(), 1);

        let key = ("vid1".to_string(), ArtifactKind::StreamUrl);
        let job = scheduler.jobs.get(&key).expect("job exists");
        assert_eq!(job.request_id, 1);
        assert_eq!(job.track_id, "vid1");
        assert_eq!(job.artifact, ArtifactKind::StreamUrl);
        assert_eq!(job.tier, PreloadTier::Eager);
        assert_eq!(job.status, JobStatus::Pending);
        assert!(scheduler.eager_lane.contains(&key));
    }

    #[test]
    fn test_submit_dedup_escalates_priority() {
        let mut scheduler = PreloadScheduler::new();
        scheduler.submit(request(1, "vid1", ArtifactKind::AudioPrefix, PreloadTier::Background));
        scheduler.submit(request(2, "vid1", ArtifactKind::AudioPrefix, PreloadTier::Immediate));

        assert_eq!(scheduler.jobs.len(), 1);

        let key = ("vid1".to_string(), ArtifactKind::AudioPrefix);
        let job = scheduler.jobs.get(&key).expect("job exists");
        assert_eq!(job.request_id, 2);
        assert_eq!(job.tier, PreloadTier::Immediate);

        assert!(scheduler.immediate_lane.contains(&key));
        assert!(!scheduler.background_lane.contains(&key));
    }

    #[test]
    fn test_cancel_request_removes_jobs() {
        let mut scheduler = PreloadScheduler::new();
        scheduler.submit(request(10, "vid1", ArtifactKind::StreamUrl, PreloadTier::Gapless));
        scheduler.submit(request(10, "vid1", ArtifactKind::AudioPrefix, PreloadTier::Gapless));
        scheduler.submit(request(11, "vid2", ArtifactKind::StreamUrl, PreloadTier::Eager));

        scheduler.cancel_request(10);

        assert_eq!(scheduler.jobs.len(), 1);
        let remaining_key = ("vid2".to_string(), ArtifactKind::StreamUrl);
        assert!(scheduler.jobs.contains_key(&remaining_key));

        let removed_key_1 = ("vid1".to_string(), ArtifactKind::StreamUrl);
        let removed_key_2 = ("vid1".to_string(), ArtifactKind::AudioPrefix);
        assert!(!scheduler.jobs.contains_key(&removed_key_1));
        assert!(!scheduler.jobs.contains_key(&removed_key_2));

        assert!(!scheduler.gapless_lane.contains(&removed_key_1));
        assert!(!scheduler.gapless_lane.contains(&removed_key_2));
    }
}
