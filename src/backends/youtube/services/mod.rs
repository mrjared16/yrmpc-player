//! YouTube Music backend services
//!
//! Services split responsibilities of the YouTube server:
//! - ApiService: YouTube Music API calls
//! - PlaybackService: MPV control and stream extraction
//! - QueueService: Playback queue management

pub mod api_service;
pub mod playback_service;
pub mod queue_service;

pub use api_service::ApiService;
pub use playback_service::PlaybackService;
pub use queue_service::{QueueService, QueueItem, RepeatMode};
