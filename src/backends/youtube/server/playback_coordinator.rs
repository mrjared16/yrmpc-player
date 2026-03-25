use std::collections::HashMap;

use super::playback_horizon::ResolvedPlaybackHorizon;

const NEXT_THREE_WINDOW_SIZE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOwner {
    ImmediateRelay,
    DirectFallback,
    QueueExtract,
    QueuePrefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackJobState {
    None,
    Extracting,
    Extracted,
    Prefixing,
    PrefixReady,
    PlayingRelay,
    PlayingDirect,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackCoordinatorSnapshot {
    pub current_track: Option<String>,
    pub current_owner: Option<TrackOwner>,
    pub resolved_horizon: Vec<String>,
    pub next_three_window: Vec<String>,
    pub active_prefix_job: Option<String>,
    pub track_states: HashMap<String, TrackJobState>,
    pub playback_started: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackCoordinator {
    snapshot: PlaybackCoordinatorSnapshot,
}

impl PlaybackCoordinator {
    #[must_use]
    pub fn snapshot(&self) -> PlaybackCoordinatorSnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn current_track_id(&self) -> Option<String> {
        self.snapshot.current_track.clone()
    }

    #[must_use]
    pub fn current_owner(&self) -> Option<TrackOwner> {
        self.snapshot.current_owner
    }

    #[must_use]
    pub fn playback_started(&self) -> bool {
        self.snapshot.playback_started
    }

    #[must_use]
    pub fn has_pending_current_track_selection(&self) -> bool {
        self.snapshot.current_owner.is_some() && !self.snapshot.playback_started
    }

    #[must_use]
    pub fn next_three_window(&self) -> Vec<String> {
        self.snapshot.next_three_window.clone()
    }

    #[must_use]
    pub fn resolved_horizon(&self) -> Vec<String> {
        self.snapshot.resolved_horizon.clone()
    }

    #[must_use]
    pub fn track_state(&self, track_id: &str) -> Option<TrackJobState> {
        self.snapshot.track_states.get(track_id).copied()
    }

    pub fn begin_immediate_play(&mut self, track_id: impl Into<String>) {
        let track_id = track_id.into();
        self.snapshot.current_track = Some(track_id.clone());
        self.snapshot.current_owner = Some(TrackOwner::ImmediateRelay);
        self.snapshot.playback_started = false;
        self.snapshot.active_prefix_job = None;
        self.snapshot.next_three_window.clear();
        self.snapshot.track_states.insert(track_id, TrackJobState::Extracting);
    }

    pub fn reset(&mut self) {
        self.snapshot = PlaybackCoordinatorSnapshot::default();
    }

    pub fn mark_bytes_started(&mut self, track_id: &str) {
        if self.snapshot.current_track.as_deref() != Some(track_id) {
            return;
        }

        self.snapshot.playback_started = true;
        let job_state = match self.snapshot.current_owner {
            Some(TrackOwner::DirectFallback) => TrackJobState::PlayingDirect,
            _ => TrackJobState::PlayingRelay,
        };
        self.snapshot.track_states.insert(track_id.to_string(), job_state);

        if self.snapshot.active_prefix_job.is_none() {
            self.recompute_next_three_window();
        }
    }

    pub fn swap_current_track_to_direct_fallback(&mut self, track_id: &str) -> bool {
        if self.snapshot.current_track.as_deref() != Some(track_id) {
            return false;
        }

        self.snapshot.current_owner = Some(TrackOwner::DirectFallback);
        if self.snapshot.playback_started {
            self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::PlayingDirect);
        }
        true
    }

    pub fn queue_changed(&mut self, new_horizon: ResolvedPlaybackHorizon) {
        self.snapshot.resolved_horizon = new_horizon.track_ids().to_vec();

        if self.snapshot.playback_started && self.snapshot.active_prefix_job.is_none() {
            self.recompute_next_three_window();
        }
    }

    pub fn sync_current_track_from_queue(
        &mut self,
        current_track: Option<String>,
        new_horizon: ResolvedPlaybackHorizon,
    ) {
        let track_changed = self.snapshot.current_track != current_track;
        self.snapshot.current_track = current_track;
        if track_changed {
            self.snapshot.current_owner = None;
        }
        self.queue_changed(new_horizon);
    }

    #[must_use]
    pub fn should_preserve_pending_current_track(&self, observed_current_track: Option<&str>) -> bool {
        self.has_pending_current_track_selection()
            && self.snapshot.current_track.as_deref() != observed_current_track
    }

    pub fn sync_with_queue_observation(
        &mut self,
        observed_current_track: Option<String>,
        new_horizon: ResolvedPlaybackHorizon,
    ) {
        if self.should_preserve_pending_current_track(observed_current_track.as_deref()) {
            self.queue_changed(new_horizon);
            return;
        }

        if observed_current_track.is_some() {
            self.sync_current_track_from_queue(observed_current_track, new_horizon);
        } else {
            self.queue_changed(new_horizon);
        }
    }

    pub fn claim_next_prefix_job(&mut self) -> Option<String> {
        if !self.snapshot.playback_started || self.snapshot.active_prefix_job.is_some() {
            return None;
        }

        let next_track = self
            .snapshot
            .next_three_window
            .iter()
            .find(|track_id| {
                !matches!(
                    self.snapshot.track_states.get(track_id.as_str()),
                    Some(
                        TrackJobState::PrefixReady
                            | TrackJobState::Prefixing
                            | TrackJobState::Failed
                    )
                )
            })
            .cloned()?;

        self.snapshot.active_prefix_job = Some(next_track.clone());
        self.snapshot.track_states.insert(next_track.clone(), TrackJobState::Prefixing);
        Some(next_track)
    }

    pub fn revalidate_claimed_prefix_job(&mut self, track_id: &str) -> bool {
        if self.snapshot.active_prefix_job.as_deref() != Some(track_id) {
            return false;
        }

        let still_valid = self.snapshot.playback_started
            && self.snapshot.current_track.as_deref() != Some(track_id)
            && self
                .snapshot
                .next_three_window
                .iter()
                .any(|candidate| candidate == track_id);

        if !still_valid {
            self.snapshot.active_prefix_job = None;
            if matches!(self.snapshot.track_states.get(track_id), Some(TrackJobState::Prefixing)) {
                self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::None);
            }
            self.recompute_next_three_window();
        }

        still_valid
    }

    pub fn finish_prefix_job(&mut self, track_id: &str) -> bool {
        if self.snapshot.active_prefix_job.as_deref() != Some(track_id) {
            return false;
        }

        self.snapshot.active_prefix_job = None;
        self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::PrefixReady);
        self.recompute_next_three_window();
        true
    }

    pub fn fail_prefix_job(&mut self, track_id: &str) -> bool {
        if self.snapshot.active_prefix_job.as_deref() != Some(track_id) {
            return false;
        }

        self.snapshot.active_prefix_job = None;
        self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::Failed);
        self.recompute_next_three_window();
        true
    }

    #[must_use]
    pub fn should_accept_queue_extract_result(&self, track_id: &str) -> bool {
        self.snapshot.current_track.as_deref() != Some(track_id)
    }

    fn recompute_next_three_window(&mut self) {
        let horizon = ResolvedPlaybackHorizon::new(self.snapshot.resolved_horizon.clone());
        self.snapshot.next_three_window =
            horizon.next_tracks_after(self.snapshot.current_track.as_deref(), NEXT_THREE_WINDOW_SIZE);
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaybackCoordinator, TrackJobState};
    use crate::backends::youtube::server::playback_horizon::ResolvedPlaybackHorizon;

    fn horizon(track_ids: &[&str]) -> ResolvedPlaybackHorizon {
        ResolvedPlaybackHorizon::new(track_ids.iter().map(|track_id| (*track_id).to_string()).collect())
    }

    #[test]
    fn queue_changes_recompute_window_after_bytes_start() {
        let mut coordinator = PlaybackCoordinator::default();

        coordinator.queue_changed(horizon(&["current", "a", "b", "c"]));
        coordinator.begin_immediate_play("current");
        assert!(coordinator.snapshot().next_three_window.is_empty());

        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.snapshot().next_three_window, vec!["a", "b", "c"]);
    }

    #[test]
    fn claim_next_prefix_job_skips_ready_tracks() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b", "c"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("a"));
        assert!(coordinator.finish_prefix_job("a"));

        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("b"));
        assert_eq!(coordinator.snapshot().track_states.get("a"), Some(&TrackJobState::PrefixReady));
    }

    #[test]
    fn failed_prefix_job_is_not_immediately_retried() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b", "c"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("a"));
        assert!(coordinator.fail_prefix_job("a"));
        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("b"));
        assert_eq!(coordinator.track_state("a"), Some(TrackJobState::Failed));
    }

    #[test]
    fn sync_current_track_refreshes_window_for_new_current_track() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b", "c"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.next_three_window(), vec!["a", "b", "c"]);

        coordinator.sync_current_track_from_queue(
            Some("a".to_string()),
            horizon(&["current", "a", "b", "c"]),
        );

        assert_eq!(coordinator.current_track_id().as_deref(), Some("a"));
        assert_eq!(coordinator.current_owner(), None);
        assert_eq!(coordinator.next_three_window(), vec!["b", "c"]);
    }

    #[test]
    fn sync_current_track_keeps_owner_when_track_is_unchanged() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        coordinator.sync_current_track_from_queue(
            Some("current".to_string()),
            horizon(&["current", "a", "b"]),
        );

        assert_eq!(coordinator.current_owner(), Some(super::TrackOwner::ImmediateRelay));
        assert_eq!(coordinator.next_three_window(), vec!["a", "b"]);
    }

    #[test]
    fn reset_clears_current_track_and_playback_state() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("a"));

        coordinator.reset();

        assert_eq!(coordinator.current_track_id(), None);
        assert_eq!(coordinator.current_owner(), None);
        assert!(!coordinator.playback_started());
        assert!(coordinator.next_three_window().is_empty());
        assert_eq!(coordinator.claim_next_prefix_job(), None);
    }

    #[test]
    fn revalidate_claimed_prefix_job_drops_track_that_became_current() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "next", "later"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("next"));

        coordinator.begin_immediate_play("next");

        assert!(!coordinator.revalidate_claimed_prefix_job("next"));
        assert_eq!(coordinator.snapshot().active_prefix_job, None);
        assert_eq!(coordinator.track_state("next"), Some(TrackJobState::Extracting));
    }

    #[test]
    fn queue_extract_results_always_reject_current_track() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        coordinator.sync_current_track_from_queue(
            Some("current".to_string()),
            horizon(&["current", "a", "b"]),
        );

        assert!(!coordinator.should_accept_queue_extract_result("current"));
        assert!(coordinator.should_accept_queue_extract_result("a"));
    }

    #[test]
    fn sync_with_queue_observation_preserves_pending_current_track_on_conflict() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["old-current", "next"]));
        coordinator.begin_immediate_play("new-current");

        coordinator.sync_with_queue_observation(
            Some("old-current".to_string()),
            horizon(&["new-current", "next", "later"]),
        );

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("new-current"));
        assert_eq!(snapshot.current_owner, Some(super::TrackOwner::ImmediateRelay));
        assert_eq!(snapshot.resolved_horizon, vec!["new-current", "next", "later"]);
    }

    #[test]
    fn sync_with_queue_observation_keeps_current_track_when_observation_is_missing() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(horizon(&["current", "a", "b"]));
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        coordinator.sync_with_queue_observation(None, horizon(&["current", "a", "b", "c"]));

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("current"));
        assert_eq!(snapshot.current_owner, Some(super::TrackOwner::ImmediateRelay));
        assert_eq!(snapshot.resolved_horizon, vec!["current", "a", "b", "c"]);
    }
}
