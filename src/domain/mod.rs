// Domain models - backend-agnostic types
// These types are used throughout the UI and can be populated from any backend

pub mod song;
pub mod status;
pub mod queue;

// Re-export main types for convenience
pub use song::Song;
pub use status::{Status, State as PlaybackState};
pub use queue::QueuePosition;
