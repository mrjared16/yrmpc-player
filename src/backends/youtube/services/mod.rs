//! YouTube Music backend services
//!
//! Services split responsibilities of the YouTube server:
//! - ApiService: YouTube Music API calls
//! - PlaybackService: MPV control and stream extraction
//! - QueueService: Playback queue management
//! - AudioPrefetcher: Rate-limited audio prefetching with priority queue

pub mod api_service;
pub mod audio_prefetcher;
pub mod internal_event;
pub mod playback_service;
pub mod playback_state;
pub mod preload_scheduler;
pub mod preparer;
pub mod queue_service;
pub mod registry;

pub use api_service::ApiService;
pub use internal_event::InternalEvent;
pub use playback_service::PlaybackService;
pub use playback_state::{AdvanceIntent, PlaybackState, PlaybackStateTracker};
pub use preload_scheduler::{ArtifactKind, PreloadRequest, PreloadScheduler, TrackId};
pub use preparer::{PlaybackMode, PlaybackPreparer, PlaybackPreparerConfig, PreparedPlayback};
pub use queue_service::{QueueItem, QueueService, RepeatMode};
pub use registry::YouTubeServices;
