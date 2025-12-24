// Domain models - backend-agnostic types
// These types are used throughout the UI and can be populated from any backend

pub mod song;
pub mod status;
pub mod queue;
pub mod search;
pub mod display;
pub mod actions;
pub mod details;
pub mod content;

// Re-export main types for convenience
pub use song::Song;
pub use status::{Status, State as PlaybackState, OnOffOneshot};
pub use queue::QueuePosition;
pub use search::{SearchItem, PlayableItem, BrowsableItem, Displayable, ItemAction, QueueCapability, QueueAction};
pub use actions::{QueueItemAction, QueueItemOps, ItemContext};

// Detail types for content views (legacy - being replaced by content module)
pub use details::{
    ArtistRef, AlbumRef, PlaylistRef,
    AlbumDetails, ArtistDetails, PlaylistDetails,
    ContentDetails,
};

// New Type-Safe Hybrid content types
pub use content::{
    ContentDetails as ContentDetailsV2,
    AlbumContent, ArtistContent, PlaylistContent,
    ContentRef, ContentType, ReleaseType,
    Extensions, ExtensionsBuilder, Section, SectionKey, SectionData,
    Stat, StatKey, StatValue,
    Action, ActionKind,
};
