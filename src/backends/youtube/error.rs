//! YouTube backend error types

use std::path::PathBuf;

/// YouTube backend errors
#[derive(Debug, thiserror::Error)]
pub enum YouTubeError {
    #[error("Daemon not running at {0}")]
    DaemonNotRunning(PathBuf),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("YouTube API error: {0}")]
    ApiError(String),

    #[error("Song not found with ID: {0}")]
    SongNotFound(u32),

    #[error("Playback error: {0}")]
    PlaybackError(String),

    #[error("Queue error: {0}")]
    QueueError(String),

    #[error("MPV error: {0}")]
    MpvError(String),

    #[error("Stream extraction failed: {0}")]
    StreamError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, YouTubeError>;
