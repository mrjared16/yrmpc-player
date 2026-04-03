use std::collections::HashMap;

use crate::backends::youtube::config::BackgroundExtractMode;

use super::playback_horizon::ResolvedPlaybackHorizon;

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
    pub extract_scope: Vec<String>,
    pub prefix_window: Vec<String>,
    pub active_prefix_job: Option<String>,
    pub track_states: HashMap<String, TrackJobState>,
    pub playback_started: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparationPlan {
    pub current_track: Option<String>,
    pub playback_started: bool,
    pub resolved_horizon: Vec<String>,
    pub extract_scope_generation: u64,
    pub extract_scope: Vec<String>,
    pub prefix_window: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlaybackCoordinator {
    snapshot: PlaybackCoordinatorSnapshot,
    queue_track_ids: Vec<String>,
    background_extract_mode: BackgroundExtractMode,
    future_track_count: usize,
    extract_scope_generation: u64,
}

impl PlaybackCoordinator {
    #[must_use]
    pub fn new(background_extract_mode: BackgroundExtractMode, future_track_count: usize) -> Self {
        Self {
            snapshot: PlaybackCoordinatorSnapshot::default(),
            queue_track_ids: Vec::new(),
            background_extract_mode,
            future_track_count,
            extract_scope_generation: 0,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> PlaybackCoordinatorSnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn preparation_plan(&self) -> PreparationPlan {
        PreparationPlan {
            current_track: self.snapshot.current_track.clone(),
            playback_started: self.snapshot.playback_started,
            resolved_horizon: self.snapshot.resolved_horizon.clone(),
            extract_scope_generation: self.extract_scope_generation,
            extract_scope: self.snapshot.extract_scope.clone(),
            prefix_window: self.snapshot.prefix_window.clone(),
        }
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
    pub fn extract_scope(&self) -> Vec<String> {
        self.snapshot.extract_scope.clone()
    }

    #[must_use]
    pub fn prefix_window(&self) -> Vec<String> {
        self.snapshot.prefix_window.clone()
    }

    #[must_use]
    pub fn extract_scope_generation(&self) -> u64 {
        self.extract_scope_generation
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
        self.begin_immediate_play_internal(track_id.into(), true);
    }

    pub fn begin_immediate_play_preserving_extract_scope(&mut self, track_id: impl Into<String>) {
        self.begin_immediate_play_internal(track_id.into(), false);
    }

    fn begin_immediate_play_internal(&mut self, track_id: String, _refresh_extract_scope: bool) {
        self.snapshot.current_track = Some(track_id.clone());
        self.snapshot.current_owner = Some(TrackOwner::ImmediateRelay);
        self.snapshot.playback_started = false;
        self.snapshot.active_prefix_job = None;
        self.snapshot.track_states.insert(track_id, TrackJobState::Extracting);
        self.recompute_prefix_window();
        log::debug!(
            "[STARTUP-POLICY] phase=begin_immediate_play current_track={} playback_started={} extract_scope_len={} prefix_window_len={}",
            self.snapshot.current_track.as_deref().unwrap_or("unknown"),
            self.snapshot.playback_started,
            self.snapshot.extract_scope.len(),
            self.snapshot.prefix_window.len(),
        );
    }

    pub fn reset(&mut self) {
        self.snapshot = PlaybackCoordinatorSnapshot::default();
        self.queue_track_ids.clear();
        self.extract_scope_generation = 0;
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

        self.recompute_prefix_window();
        log::debug!(
            "[STARTUP-POLICY] phase=playback_confirmed current_track={} owner={:?} extract_scope={:?} prefix_window={:?}",
            track_id,
            self.snapshot.current_owner,
            self.snapshot.extract_scope,
            self.snapshot.prefix_window,
        );
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

    pub fn queue_changed(
        &mut self,
        new_horizon: ResolvedPlaybackHorizon,
        queue_track_ids: Vec<String>,
    ) {
        self.snapshot.resolved_horizon = new_horizon.track_ids().to_vec();

        let queue_delta_warm_ids = newly_added_track_ids(&self.queue_track_ids, &queue_track_ids);

        if self.snapshot.extract_scope != queue_delta_warm_ids {
            self.extract_scope_generation = self.extract_scope_generation.saturating_add(1);
        }
        self.queue_track_ids = queue_track_ids;
        self.snapshot.extract_scope = queue_delta_warm_ids;
        self.recompute_prefix_window();

        log::debug!(
            "[STARTUP-POLICY] phase=queue_changed playback_started={} current_track={:?} horizon_len={} extract_scope={:?} prefix_window={:?}",
            self.snapshot.playback_started,
            self.snapshot.current_track,
            self.snapshot.resolved_horizon.len(),
            self.snapshot.extract_scope,
            self.snapshot.prefix_window,
        );
    }

    pub fn sync_current_track_from_queue(
        &mut self,
        current_track: Option<String>,
        new_horizon: ResolvedPlaybackHorizon,
        queue_track_ids: Vec<String>,
    ) {
        let track_changed = self.snapshot.current_track != current_track;
        self.snapshot.current_track = current_track;
        if track_changed {
            self.snapshot.current_owner = None;
        }
        self.queue_changed(new_horizon, queue_track_ids);
    }

    #[must_use]
    pub fn should_preserve_pending_current_track(
        &self,
        observed_current_track: Option<&str>,
    ) -> bool {
        self.has_pending_current_track_selection()
            && self.snapshot.current_track.as_deref() != observed_current_track
    }

    pub fn sync_with_queue_observation(
        &mut self,
        observed_current_track: Option<String>,
        new_horizon: ResolvedPlaybackHorizon,
        queue_track_ids: Vec<String>,
    ) {
        if self.should_preserve_pending_current_track(observed_current_track.as_deref()) {
            self.queue_changed(new_horizon, queue_track_ids);
            return;
        }

        if observed_current_track.is_some() {
            self.sync_current_track_from_queue(
                observed_current_track,
                new_horizon,
                queue_track_ids,
            );
        } else {
            self.queue_changed(new_horizon, queue_track_ids);
        }
    }

    pub fn sync_with_playback_observation(
        &mut self,
        observed_current_track: Option<String>,
        new_horizon: ResolvedPlaybackHorizon,
    ) {
        let new_resolved_horizon = new_horizon.track_ids().to_vec();
        if self.snapshot.current_track == observed_current_track
            && self.snapshot.resolved_horizon == new_resolved_horizon
        {
            return;
        }

        if self.should_preserve_pending_current_track(observed_current_track.as_deref()) {
            self.snapshot.resolved_horizon = new_resolved_horizon;
            self.recompute_prefix_window_only(&new_horizon);
            log::debug!(
                "[STARTUP-POLICY] phase=playback_observation_preserved playback_started={} current_track={:?} horizon_len={} extract_scope={:?} prefix_window={:?}",
                self.snapshot.playback_started,
                self.snapshot.current_track,
                self.snapshot.resolved_horizon.len(),
                self.snapshot.extract_scope,
                self.snapshot.prefix_window,
            );
            return;
        }

        let track_changed = self.snapshot.current_track != observed_current_track;
        self.snapshot.current_track = observed_current_track;
        if track_changed {
            self.snapshot.current_owner = None;
        }
        self.snapshot.resolved_horizon = new_resolved_horizon;

        self.recompute_prefix_window_only(&new_horizon);

        log::debug!(
            "[STARTUP-POLICY] phase=playback_observation playback_started={} current_track={:?} horizon_len={} extract_scope={:?} prefix_window={:?}",
            self.snapshot.playback_started,
            self.snapshot.current_track,
            self.snapshot.resolved_horizon.len(),
            self.snapshot.extract_scope,
            self.snapshot.prefix_window,
        );
    }

    pub fn claim_next_prefix_job(&mut self) -> Option<String> {
        if !self.snapshot.playback_started || self.snapshot.active_prefix_job.is_some() {
            return None;
        }

        let next_track = self
            .snapshot
            .prefix_window
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
            && self.snapshot.prefix_window.iter().any(|candidate| candidate == track_id);

        if !still_valid {
            self.snapshot.active_prefix_job = None;
            if matches!(self.snapshot.track_states.get(track_id), Some(TrackJobState::Prefixing)) {
                self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::None);
            }
            self.recompute_prefix_window();
        }

        still_valid
    }

    pub fn finish_prefix_job(&mut self, track_id: &str) -> bool {
        if self.snapshot.active_prefix_job.as_deref() != Some(track_id) {
            return false;
        }

        self.snapshot.active_prefix_job = None;
        self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::PrefixReady);
        self.recompute_prefix_window();
        true
    }

    pub fn fail_prefix_job(&mut self, track_id: &str) -> bool {
        if self.snapshot.active_prefix_job.as_deref() != Some(track_id) {
            return false;
        }

        self.snapshot.active_prefix_job = None;
        self.snapshot.track_states.insert(track_id.to_string(), TrackJobState::Failed);
        self.recompute_prefix_window();
        true
    }

    #[must_use]
    pub fn should_accept_queue_extract_result(&self, track_id: &str) -> bool {
        self.snapshot.current_track.as_deref() != Some(track_id)
    }

    fn recompute_prefix_window(&mut self) {
        let current_track = self.snapshot.current_track.as_deref();

        let horizon = ResolvedPlaybackHorizon::new(self.snapshot.resolved_horizon.clone());
        let reserved_prefix_tracks =
            horizon.next_tracks_after(current_track, self.future_track_count);
        self.snapshot.prefix_window =
            if self.snapshot.playback_started { reserved_prefix_tracks } else { Vec::new() };
    }

    fn recompute_prefix_window_only(&mut self, horizon: &ResolvedPlaybackHorizon) {
        let current_track = self.snapshot.current_track.as_deref();
        self.snapshot.prefix_window = if self.snapshot.playback_started {
            horizon.next_tracks_after(current_track, self.future_track_count)
        } else {
            Vec::new()
        };
    }
}

fn newly_added_track_ids(previous: &[String], next: &[String]) -> Vec<String> {
    next.iter().filter(|track_id| !previous.contains(track_id)).cloned().collect()
}

impl Default for PlaybackCoordinator {
    fn default() -> Self {
        Self::new(BackgroundExtractMode::Balanced, 2)
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaybackCoordinator, TrackJobState};
    use crate::backends::youtube::config::BackgroundExtractMode;
    use crate::backends::youtube::server::playback_horizon::ResolvedPlaybackHorizon;

    fn horizon(track_ids: &[&str]) -> ResolvedPlaybackHorizon {
        ResolvedPlaybackHorizon::new(
            track_ids.iter().map(|track_id| (*track_id).to_string()).collect(),
        )
    }

    fn queue_track_ids(track_ids: &[&str]) -> Vec<String> {
        track_ids.iter().map(|track_id| (*track_id).to_string()).collect()
    }

    #[test]
    fn queue_changes_emit_newly_added_warm_ids_and_prefix_after_bytes_start() {
        let mut coordinator = PlaybackCoordinator::default();

        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );
        coordinator.begin_immediate_play("current");
        assert_eq!(coordinator.snapshot().extract_scope, vec!["current", "a", "b", "c"]);
        assert!(coordinator.snapshot().prefix_window.is_empty());

        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.snapshot().extract_scope, vec!["current", "a", "b", "c"]);
        assert_eq!(coordinator.snapshot().prefix_window, vec!["a", "b"]);
    }

    #[test]
    fn performance_mode_keeps_queue_warm_ids_independent_of_prefix_window() {
        let mut coordinator = PlaybackCoordinator::new(BackgroundExtractMode::Performance, 2);

        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c", "d"]),
            queue_track_ids(&["current", "a", "b", "c", "d"]),
        );
        coordinator.begin_immediate_play("current");

        assert_eq!(coordinator.snapshot().extract_scope, vec!["current", "a", "b", "c", "d"]);
        assert!(coordinator.snapshot().prefix_window.is_empty());

        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.snapshot().extract_scope, vec!["current", "a", "b", "c", "d"]);
        assert_eq!(coordinator.snapshot().prefix_window, vec!["a", "b"]);
    }

    #[test]
    fn performance_playback_observation_updates_prefix_without_refreshing_extract_scope() {
        let mut coordinator = PlaybackCoordinator::new(BackgroundExtractMode::Performance, 2);

        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        assert_eq!(coordinator.extract_scope(), vec!["current", "a", "b", "c"]);
        assert_eq!(coordinator.prefix_window(), vec!["a", "b"]);

        coordinator.sync_with_playback_observation(
            Some("a".to_string()),
            horizon(&["a", "b", "c", "current"]),
        );

        assert_eq!(coordinator.extract_scope(), vec!["current", "a", "b", "c"]);
        assert_eq!(coordinator.prefix_window(), vec!["b", "c"]);
        assert_eq!(coordinator.extract_scope_generation(), 1);
    }

    #[test]
    fn preparation_plan_returns_value_style_planning_state() {
        let mut coordinator = PlaybackCoordinator::new(BackgroundExtractMode::Balanced, 2);

        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("a"));

        let plan = coordinator.preparation_plan();

        assert_eq!(plan.current_track.as_deref(), Some("current"));
        assert!(plan.playback_started);
        assert_eq!(plan.resolved_horizon, vec!["current", "a", "b"]);
        assert_eq!(plan.extract_scope_generation, 1);
        assert_eq!(plan.extract_scope, vec!["current", "a", "b"]);
        assert_eq!(plan.prefix_window, vec!["a", "b"]);
    }

    #[test]
    fn claim_next_prefix_job_skips_ready_tracks() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );
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
        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );
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
        coordinator.queue_changed(
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.extract_scope(), vec!["current", "a", "b", "c"]);
        assert_eq!(coordinator.prefix_window(), vec!["a", "b"]);

        coordinator.sync_current_track_from_queue(
            Some("a".to_string()),
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );

        assert_eq!(coordinator.current_track_id().as_deref(), Some("a"));
        assert_eq!(coordinator.current_owner(), None);
        assert!(coordinator.extract_scope().is_empty());
        assert_eq!(coordinator.prefix_window(), vec!["b", "c"]);
    }

    #[test]
    fn sync_current_track_keeps_owner_when_track_is_unchanged() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        coordinator.sync_current_track_from_queue(
            Some("current".to_string()),
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );

        assert_eq!(coordinator.current_owner(), Some(super::TrackOwner::ImmediateRelay));
        assert!(coordinator.extract_scope().is_empty());
        assert_eq!(coordinator.prefix_window(), vec!["a", "b"]);
    }

    #[test]
    fn reset_clears_current_track_and_playback_state() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("a"));

        coordinator.reset();

        assert_eq!(coordinator.current_track_id(), None);
        assert_eq!(coordinator.current_owner(), None);
        assert!(!coordinator.playback_started());
        assert!(coordinator.extract_scope().is_empty());
        assert!(coordinator.prefix_window().is_empty());
        assert_eq!(coordinator.claim_next_prefix_job(), None);
    }

    #[test]
    fn revalidate_claimed_prefix_job_drops_track_that_became_current() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["current", "next", "later"]),
            queue_track_ids(&["current", "next", "later"]),
        );
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
        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");
        coordinator.sync_current_track_from_queue(
            Some("current".to_string()),
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );

        assert!(!coordinator.should_accept_queue_extract_result("current"));
        assert!(coordinator.should_accept_queue_extract_result("a"));
    }

    #[test]
    fn sync_with_queue_observation_preserves_pending_current_track_on_conflict() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["old-current", "next"]),
            queue_track_ids(&["old-current", "next"]),
        );
        coordinator.begin_immediate_play("new-current");

        coordinator.sync_with_queue_observation(
            Some("old-current".to_string()),
            horizon(&["new-current", "next", "later"]),
            queue_track_ids(&["new-current", "next", "later"]),
        );

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("new-current"));
        assert_eq!(snapshot.current_owner, Some(super::TrackOwner::ImmediateRelay));
        assert_eq!(snapshot.resolved_horizon, vec!["new-current", "next", "later"]);
    }

    #[test]
    fn sync_with_queue_observation_keeps_current_track_when_observation_is_missing() {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        coordinator.begin_immediate_play("current");
        coordinator.mark_bytes_started("current");

        coordinator.sync_with_queue_observation(
            None,
            horizon(&["current", "a", "b", "c"]),
            queue_track_ids(&["current", "a", "b", "c"]),
        );

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("current"));
        assert_eq!(snapshot.current_owner, Some(super::TrackOwner::ImmediateRelay));
        assert_eq!(snapshot.resolved_horizon, vec!["current", "a", "b", "c"]);
    }

    #[test]
    fn begin_immediate_play_preserves_queue_delta_warm_ids_and_clears_prefix_window_until_playback_starts()
     {
        let mut coordinator = PlaybackCoordinator::default();
        coordinator.queue_changed(
            horizon(&["old", "a", "b", "c"]),
            queue_track_ids(&["old", "a", "b", "c"]),
        );
        coordinator.begin_immediate_play("old");
        coordinator.mark_bytes_started("old");
        assert_eq!(coordinator.extract_scope(), vec!["old", "a", "b", "c"]);
        assert_eq!(coordinator.prefix_window(), vec!["a", "b"]);

        coordinator.begin_immediate_play("new");

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_track.as_deref(), Some("new"));
        assert!(!snapshot.playback_started);
        assert_eq!(snapshot.extract_scope, vec!["old", "a", "b", "c"]);
        assert!(snapshot.prefix_window.is_empty());
    }

    #[test]
    fn queue_changed_only_emits_newly_added_ids() {
        let mut coordinator = PlaybackCoordinator::new(BackgroundExtractMode::Performance, 2);

        coordinator.queue_changed(
            horizon(&["current", "a", "b"]),
            queue_track_ids(&["current", "a", "b"]),
        );
        assert_eq!(coordinator.extract_scope(), vec!["current", "a", "b"]);
        let generation = coordinator.extract_scope_generation();

        coordinator.queue_changed(
            horizon(&["current", "x", "a", "b"]),
            queue_track_ids(&["current", "x", "a", "b"]),
        );
        assert_eq!(coordinator.extract_scope(), vec!["x"]);
        assert!(coordinator.extract_scope_generation() > generation);

        coordinator.queue_changed(
            horizon(&["current", "a", "x", "b"]),
            queue_track_ids(&["current", "a", "x", "b"]),
        );
        assert!(coordinator.extract_scope().is_empty());
    }
}
