//! YouTube Music backend services
//!
//! Services split responsibilities of the YouTube server:
//! - ApiService: YouTube Music API calls
//! - PlaybackService: MPV control and stream extraction
//! - QueueService: Playback queue management

pub mod api_service;
pub mod internal_event;
pub mod playback_service;
pub mod playback_state;
pub mod queue_service;

pub use api_service::ApiService;
pub use internal_event::InternalEvent;
pub use playback_service::PlaybackService;
pub use playback_state::{AdvanceIntent, PlaybackState, PlaybackStateTracker};
pub use queue_service::{QueueItem, QueueService, RepeatMode};
