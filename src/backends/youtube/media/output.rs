//! MPV input building trait

use std::path::Path;

use super::StreamInfo;

/// Builds MPV input from resolved stream and optional cached audio
pub trait MpvInputBuilder: Send + Sync {
    /// Build MPV input (concat file or direct URL)
    fn build(&self, track_id: &str, stream: &StreamInfo, cached_path: Option<&Path>) -> MpvInput;
}

/// MPV input formats
#[derive(Debug, Clone)]
pub enum MpvInput {
    /// FFmpeg concat file path (cached prefix + streaming remainder)
    ConcatFile(std::path::PathBuf),
    /// Direct streaming URL (no cache)
    DirectUrl(String),
    /// Local file path
    LocalPath(std::path::PathBuf),
}
