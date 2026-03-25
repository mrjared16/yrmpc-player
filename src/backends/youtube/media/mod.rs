//! Media preparation traits for backend-agnostic playback
//!
//! Design: Functional core, imperative shell
//! - Traits = pure interfaces
//! - Implementations = handle IO

mod job_registry;
mod loader;
mod output;
mod preparer;
mod relay;
mod relay_planner;
mod relay_runtime;
mod resolver;
mod staging_pipeline;
mod upstream_plan;

use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
pub use loader::AudioLoader;
pub use output::{MpvInput, MpvInputBuilder};
pub use preparer::{CacheRequest, PrepareResult, YouTubeMediaPreparer, YouTubeMediaPreparerHandle};
pub use relay::{
    RelayByteRange, RelayContractError, RelayPlayerEndpoint, RelayRangeError,
    RelayRangePolicy, RelayReconnectOwner, RelayResponsePlan, RelaySessionId, RelaySessionSpec,
    RelaySessionState, RelayStagedArtifact, RelayTeePrefix, RelayTransportContract,
    RelayUpstreamStream,
};
pub use relay_planner::{RelayPlayStrategy, RelayPlanner};
pub use relay_runtime::RelayRuntime;
pub use resolver::{AudioFormat, StreamInfo, StreamResolver};
pub use upstream_plan::{UpstreamReadPlan, default_upstream_read_plans};

// Re-export existing PreloadTier
pub use crate::backends::youtube::protocol::play_intent::PreloadTier;

/// Preparation status
#[derive(Debug, Clone)]
pub enum PrepareStatus {
    Unknown,
    Pending { tier: PreloadTier },
    InProgress { started: Instant, tier: PreloadTier },
    Ready,
    Failed { error: String },
}

/// Core trait for preparing media for playback
///
/// This is the single authoritative entry point for media preparation.
/// All playback paths MUST go through this trait - no bypassing allowed.
///
/// # Design Principles
/// - Intent-driven: caller specifies WHAT (track + urgency), impl decides HOW
/// - Backend-agnostic: YouTube, Spotify, Local can all implement this
/// - Single choke point: prevents duplicate URL resolution / caching logic
#[async_trait]
pub trait MediaPreparer: Send + Sync {
    /// Prepare a track for playback (blocking until ready or timeout)
    ///
    /// Returns PreparedMedia which can be directly passed to MPV.
    /// For Immediate tier, may timeout and return Direct URL as fallback.
    async fn prepare(&self, track_id: &str, tier: PreloadTier) -> Result<PreparedMedia>;

    /// Prefetch a track in background (fire-and-forget)
    ///
    /// Use for queue lookahead, gapless preparation, etc.
    /// Does not block - preparation happens asynchronously.
    fn prefetch(&self, track_id: &str, tier: PreloadTier);

    fn warm(&self, track_id: &str) {
        self.prefetch(track_id, PreloadTier::Background);
    }

    fn warm_many(&self, track_ids: &[String]) {
        for track_id in track_ids {
            self.warm(track_id);
        }
    }

    fn invalidate(&self, _track_id: &str) {}

    fn activate_playback_window(&self, _track_ids: &[String]) {}
}

/// Prepared media result
#[derive(Debug, Clone)]
pub enum PreparedMedia {
    StagedPrefix {
        path: std::path::PathBuf,
        bytes: u64,
        url: String,
        content_length: u64,
    },
    /// Relay streams from byte 0 in a single connection, tees first
    /// `prefix_size` bytes to `prefix_path` on disk, and pipes everything
    /// to MPV.  Used on cache miss to avoid a separate prefix download
    /// (which triggers YouTube CDN rate-limiting on the subsequent relay
    /// upstream request).
    StreamAndCache {
        url: String,
        content_length: u64,
        prefix_path: std::path::PathBuf,
        prefix_size: u64,
    },
    Direct { url: String },
    LocalFile { path: std::path::PathBuf },
}

impl PreparedMedia {
    /// Convert to MPV-compatible URL/path string
    pub fn to_mpv_url(&self) -> String {
        match self {
            PreparedMedia::StagedPrefix { path, bytes, url, content_length } => {
                if bytes >= content_length {
                    path.display().to_string()
                } else {
                    format!(
                        "lavf://concat:{}|subfile,,start,{},end,0,,:{}",
                        path.display(),
                        bytes,
                        url
                    )
                }
            }
            PreparedMedia::Direct { url } => url.clone(),
            PreparedMedia::StreamAndCache { url, .. } => url.clone(),
            PreparedMedia::LocalFile { path } => path.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn prepared_media_direct_url_conversion() {
        let media = PreparedMedia::Direct { url: "https://example.com/audio.mp3".to_string() };
        assert_eq!(media.to_mpv_url(), "https://example.com/audio.mp3");
    }

    #[test]
    fn prepared_media_staged_prefix_url_conversion() {
        let media = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 2048,
            url: "https://example.com/stream".to_string(),
            content_length: 4096,
        };
        assert_eq!(
            media.to_mpv_url(),
            "lavf://concat:/tmp/prefix.webm|subfile,,start,2048,end,0,,:https://example.com/stream"
        );
    }

    #[test]
    fn prepared_media_staged_prefix_uses_local_file_when_fully_cached() {
        let media = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/full.webm"),
            bytes: 4096,
            url: "https://example.com/stream".to_string(),
            content_length: 4096,
        };
        assert_eq!(media.to_mpv_url(), "/tmp/full.webm");
    }

    #[test]
    fn prepared_media_local_file_conversion() {
        let media = PreparedMedia::LocalFile { path: PathBuf::from("/music/song.mp3") };
        assert_eq!(media.to_mpv_url(), "/music/song.mp3");
    }

    #[test]
    fn prepared_media_stream_and_cache_uses_upstream_url() {
        let media = PreparedMedia::StreamAndCache {
            url: "https://example.com/stream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 1024,
        };
        assert_eq!(media.to_mpv_url(), "https://example.com/stream");
    }
}
