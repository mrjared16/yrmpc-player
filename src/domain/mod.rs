// Domain models - backend-agnostic types
// These types are used throughout the UI and can be populated from any backend

pub mod song;
pub mod status;
pub mod queue;
pub mod search;
pub mod display;
pub mod actions;
pub mod content;
pub mod detail_item;

// Re-export main types for convenience
pub use song::Song;
pub use status::{Status, State as PlaybackState, OnOffOneshot};
pub use queue::QueuePosition;
pub use search::{SearchItem, PlayableItem, BrowsableItem, Displayable, ItemAction, QueueCapability, QueueAction};
pub use actions::{QueueItemAction, QueueItemOps, ItemContext};

// Type-Safe Hybrid content types for detail views
pub use content::{
    ContentDetails,
    AlbumContent, ArtistContent, PlaylistContent, SearchResultsContent, QueueContent, SearchableContent,
    ContentRef, ContentType, ReleaseType,
    Extensions, ExtensionsBuilder, Section, SectionKey, SectionData,
    Stat, StatKey, StatValue,
    Action, ActionKind,
    ContentViewable,
};

// DetailItem - unified item type for navigation stacks
pub use detail_item::DetailItem;
