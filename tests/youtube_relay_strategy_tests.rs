use std::path::PathBuf;

use rmpc::backends::youtube::media::{
    PreparedMedia, RelayPlayStrategy, RelayPlanner, UpstreamReadPlan,
};

#[test]
fn prefix_hit_plans_cache_hit_relay() {
    let strategy = RelayPlanner::from_prepared(
        "track-123",
        &PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 2048,
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
        },
    )
    .unwrap();

    match strategy {
        RelayPlayStrategy::CacheHitRelay { track_id, stream_url, prefix } => {
            assert_eq!(track_id, "track-123");
            assert_eq!(stream_url, "https://example.com/upstream");
            assert_eq!(prefix.path, PathBuf::from("/tmp/prefix.webm"));
            assert_eq!(prefix.available.start, 0);
            assert_eq!(prefix.available.end, 2048);
        }
        other => panic!("expected cache-hit relay strategy, got {other:?}"),
    }
}

#[test]
fn immediate_miss_plans_tee_miss_relay() {
    let strategy = RelayPlanner::from_prepared(
        "track-123",
        &PreparedMedia::StreamAndCache {
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 1024,
        },
    )
    .unwrap();

    match strategy {
        RelayPlayStrategy::TeeMissRelay { track_id, stream_url, prefix_target } => {
            assert_eq!(track_id, "track-123");
            assert_eq!(stream_url, "https://example.com/upstream");
            assert_eq!(prefix_target.path, PathBuf::from("/tmp/prefix.webm"));
            assert_eq!(prefix_target.size, 1024);
        }
        other => panic!("expected tee-miss relay strategy, got {other:?}"),
    }
}

#[test]
fn direct_fallback_is_explicit_not_initial_strategy_selection() {
    let initial = RelayPlanner::from_prepared(
        "track-123",
        &PreparedMedia::StreamAndCache {
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 1024,
        },
    )
    .unwrap();

    assert!(matches!(initial, RelayPlayStrategy::TeeMissRelay { .. }));

    let fallback = RelayPlayStrategy::direct_fallback("track-123", "https://example.com/direct");
    assert!(matches!(fallback, RelayPlayStrategy::DirectFallback { .. }));
    assert_eq!(
        initial.continuation_plans(10, 20),
        vec![
            UpstreamReadPlan::QueryRange { start: 10, end: 20 },
            UpstreamReadPlan::AnchoredQueryRange { start: 10, end: 20 },
            UpstreamReadPlan::ChunkedQueryRange { start: 10, end: 20 },
        ]
    );
    assert!(fallback.continuation_plans(10, 20).is_empty());
}
