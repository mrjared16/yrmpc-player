// Domain models - backend-agnostic types
// These types are used throughout the UI and can be populated from any backend

pub mod actions;
pub mod content;
pub mod content_uri;
pub mod detail_item;
pub mod display;
pub mod media_item;
pub mod queue;
pub mod queue_entry;
pub mod search;
pub mod song;
pub mod status;

// Re-export main types for convenience
pub use actions::{ItemContext, QueueItemAction, QueueItemOps};
// Type-Safe Hybrid content types for detail views
pub use content::{
    Action, ActionKind, AlbumContent, ArtistContent, ContentDetails, ContentRef, ContentType,
    ContentViewable, Extensions, ExtensionsBuilder, PlaylistContent, QueueContent, ReleaseType,
    SearchResultsContent, SearchableContent, Section, SectionData, SectionKey, Stat, StatKey,
    StatValue,
};
// ContentUri - backend-agnostic content identification
pub use content_uri::{ContentUri, Displayable as ContentDisplayable, HasUri, UriComponents};
// DetailItem - unified item type for navigation stacks
pub use detail_item::DetailItem;
// MediaItem - strongly-typed content enum (replaces stringly-typed Song.metadata)
pub use media_item::{
    Album, Artist, BackendExtension, Displayable as MediaDisplayable, MediaItem, MpdData, Playlist,
    Track, YouTubeData,
};
pub use queue::QueuePosition;
// QueueEntry - queue item with metadata
pub use queue_entry::QueueEntry;
pub use search::{
    BrowsableItem, Displayable, ItemAction, PlayableItem, QueueAction, QueueCapability, SearchItem,
    SearchResults, SearchSection,
};
pub use song::Song;
pub use status::{OnOffOneshot, State as PlaybackState, Status};
