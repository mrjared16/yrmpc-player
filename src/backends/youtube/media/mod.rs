//! Media preparation traits for backend-agnostic playback
//!
//! Design: Functional core, imperative shell
//! - Traits = pure interfaces
//! - Implementations = handle IO

mod loader;
mod output;
mod preparer;
mod resolver;

use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
pub use loader::AudioLoader;
pub use output::{MpvInput, MpvInputBuilder};
pub use preparer::{CacheRequest, PrepareResult, YouTubeMediaPreparer, YouTubeMediaPreparerHandle};
pub use resolver::{AudioFormat, StreamInfo, StreamResolver};

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
}

/// Prepared media result
#[derive(Debug, Clone)]
pub enum PreparedMedia {
    Concat { concat_path: std::path::PathBuf },
    Direct { url: String },
    LocalFile { path: std::path::PathBuf },
}

impl PreparedMedia {
    /// Convert to MPV-compatible URL/path string
    pub fn to_mpv_url(&self) -> String {
        match self {
            PreparedMedia::Concat { concat_path } => {
                format!("concat:{}", concat_path.display())
            }
            PreparedMedia::Direct { url } => url.clone(),
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
    fn prepared_media_concat_url_conversion() {
        let media = PreparedMedia::Concat { concat_path: PathBuf::from("/tmp/concat.txt") };
        assert_eq!(media.to_mpv_url(), "concat:/tmp/concat.txt");
    }

    #[test]
    fn prepared_media_local_file_conversion() {
        let media = PreparedMedia::LocalFile { path: PathBuf::from("/music/song.mp3") };
        assert_eq!(media.to_mpv_url(), "/music/song.mp3");
    }
}
