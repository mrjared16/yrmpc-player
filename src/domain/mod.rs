// Domain models - backend-agnostic types
// These types are used throughout the UI and can be populated from any backend

pub mod song;
pub mod status;
pub mod queue;
pub mod search;

// Re-export main types for convenience
pub use song::Song;
pub use status::{Status, State as PlaybackState, OnOffOneshot};
pub use queue::QueuePosition;
pub use search::{SearchItem, PlayableItem, BrowsableItem, Displayable, ItemAction, QueueCapability, QueueAction};
