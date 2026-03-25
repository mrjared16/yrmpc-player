use rmpc::backends::youtube::server::{
    playback_coordinator::{PlaybackCoordinator, TrackJobState, TrackOwner},
    playback_horizon::ResolvedPlaybackHorizon,
};

fn horizon(track_ids: &[&str]) -> ResolvedPlaybackHorizon {
    ResolvedPlaybackHorizon::new(track_ids.iter().map(|track_id| (*track_id).to_string()).collect())
}

#[test]
fn current_track_is_excluded_from_next_three_window() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "next-1", "next-2", "next-3", "next-4"]));

    coordinator.begin_immediate_play("current");
    coordinator.mark_bytes_started("current");

    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.current_track.as_deref(), Some("current"));
    assert_eq!(snapshot.current_owner, Some(TrackOwner::ImmediateRelay));
    assert_eq!(snapshot.next_three_window, vec!["next-1", "next-2", "next-3"]);
    assert_eq!(snapshot.track_states.get("current"), Some(&TrackJobState::PlayingRelay));
}

#[test]
fn next_three_comes_from_resolved_horizon_not_raw_queue_order() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "shuffle-3", "shuffle-1", "shuffle-2", "tail"]));

    coordinator.begin_immediate_play("current");
    coordinator.mark_bytes_started("current");

    assert_eq!(
        coordinator.snapshot().next_three_window,
        vec!["shuffle-3", "shuffle-1", "shuffle-2"]
    );
}

#[test]
fn queue_change_during_prefix_job_keeps_active_job_until_finished() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "old-1", "old-2", "old-3", "old-4"]));
    coordinator.begin_immediate_play("current");
    coordinator.mark_bytes_started("current");

    assert_eq!(coordinator.claim_next_prefix_job().as_deref(), Some("old-1"));
    assert_eq!(coordinator.snapshot().active_prefix_job.as_deref(), Some("old-1"));

    coordinator.queue_changed(horizon(&["current", "new-1", "new-2", "new-3", "new-4"]));

    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.active_prefix_job.as_deref(), Some("old-1"));
    assert_eq!(snapshot.next_three_window, vec!["old-1", "old-2", "old-3"]);

    coordinator.finish_prefix_job("old-1");

    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.active_prefix_job, None);
    assert_eq!(snapshot.track_states.get("old-1"), Some(&TrackJobState::PrefixReady));
    assert_eq!(snapshot.next_three_window, vec!["new-1", "new-2", "new-3"]);
}

#[test]
fn stale_batch_extract_result_for_current_immediate_track_is_rejected() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "next-1", "next-2"]));
    coordinator.begin_immediate_play("current");

    assert!(!coordinator.should_accept_queue_extract_result("current"));
    assert!(coordinator.should_accept_queue_extract_result("next-1"));
}

#[test]
fn direct_fallback_before_playback_start_stays_direct_when_bytes_begin() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "next-1", "next-2", "next-3"]));
    coordinator.begin_immediate_play("current");

    assert!(coordinator.swap_current_track_to_direct_fallback("current"));

    coordinator.mark_bytes_started("current");

    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.current_owner, Some(TrackOwner::DirectFallback));
    assert_eq!(snapshot.track_states.get("current"), Some(&TrackJobState::PlayingDirect));
    assert_eq!(snapshot.next_three_window, vec!["next-1", "next-2", "next-3"]);
}

#[test]
fn direct_fallback_keeps_next_three_work_alive_and_blocks_current_track_cache() {
    let mut coordinator = PlaybackCoordinator::default();
    coordinator.queue_changed(horizon(&["current", "next-1", "next-2", "next-3", "next-4"]));
    coordinator.begin_immediate_play("current");
    coordinator.mark_bytes_started("current");

    assert!(coordinator.swap_current_track_to_direct_fallback("current"));

    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.current_owner, Some(TrackOwner::DirectFallback));
    assert_eq!(snapshot.track_states.get("current"), Some(&TrackJobState::PlayingDirect));
    assert_eq!(snapshot.next_three_window, vec!["next-1", "next-2", "next-3"]);
    assert!(!coordinator.should_accept_queue_extract_result("current"));
    assert!(coordinator.should_accept_queue_extract_result("next-1"));
}
