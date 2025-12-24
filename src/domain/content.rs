//! Type-Safe Hybrid ContentDetails architecture.
//!
//! This module provides backend-agnostic content detail types using a hybrid approach:
//! - **Type-specific structs** (`AlbumContent`, `ArtistContent`, `PlaylistContent`) for semantic guarantees
//! - **Extensions container** for dynamic/optional sections that vary by backend
//!
//! # Design Principles
//!
//! 1. **Core fields are required** - `id`, `title`, `tracks` are always present
//! 2. **Optional metadata uses `Option<T>`** - `thumbnail`, `year`, `description`
//! 3. **Dynamic sections via Extensions** - backends return only what they support
//! 4. **No empty vectors masquerading as missing features** - missing section = not supported
//!
//! # Example
//!
//! ```ignore
//! // YouTube returns rich content
//! let album = AlbumContent {
//!     id: "abc".into(),
//!     title: "Abbey Road".into(),
//!     artist: ContentRef::new("artist1", "The Beatles"),
//!     tracks: songs,
//!     extensions: Extensions::builder()
//!         .stats(vec![Stat::year(1969), Stat::track_count(17)])
//!         .related_albums("More by The Beatles", more_albums)
//!         .build(),
//!     ..Default::default()
//! };
//!
//! // MPD returns minimal content
//! let album = AlbumContent {
//!     id: "path/to/album".into(),
//!     title: "Abbey Road".into(),
//!     artist: ContentRef::new("", "The Beatles"),
//!     tracks: songs,
//!     extensions: Extensions::new(), // Empty - no extended content
//!     ..Default::default()
//! };
//! ```

use std::time::Duration;
use super::Song;

// =============================================================================
// CONTENT DETAILS ENUM
// =============================================================================

/// Type-safe content details with guaranteed structure per type.
///
/// Each variant has required core fields plus optional extensions.
/// UI code can pattern match for type-specific rendering.
#[derive(Debug, Clone)]
pub enum ContentDetails {
    Album(AlbumContent),
    Artist(ArtistContent),
    Playlist(PlaylistContent),
}

impl ContentDetails {
    /// Get the content ID regardless of type.
    pub fn id(&self) -> &str {
        match self {
            Self::Album(a) => &a.id,
            Self::Artist(a) => &a.id,
            Self::Playlist(p) => &p.id,
        }
    }

    /// Get the title regardless of type.
    pub fn title(&self) -> &str {
        match self {
            Self::Album(a) => &a.title,
            Self::Artist(a) => &a.name,
            Self::Playlist(p) => &p.title,
        }
    }

    /// Get the thumbnail URL if available.
    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Album(a) => a.thumbnail.as_deref(),
            Self::Artist(a) => a.thumbnail.as_deref(),
            Self::Playlist(p) => p.thumbnail.as_deref(),
        }
    }

    /// Get the primary tracks/songs.
    pub fn tracks(&self) -> &[Song] {
        match self {
            Self::Album(a) => &a.tracks,
            Self::Artist(a) => &a.top_songs,
            Self::Playlist(p) => &p.tracks,
        }
    }

    /// Get the extensions container.
    pub fn extensions(&self) -> &Extensions {
        match self {
            Self::Album(a) => &a.extensions,
            Self::Artist(a) => &a.extensions,
            Self::Playlist(p) => &p.extensions,
        }
    }

    /// Check if this is an album.
    pub fn is_album(&self) -> bool {
        matches!(self, Self::Album(_))
    }

    /// Check if this is an artist.
    pub fn is_artist(&self) -> bool {
        matches!(self, Self::Artist(_))
    }

    /// Check if this is a playlist.
    pub fn is_playlist(&self) -> bool {
        matches!(self, Self::Playlist(_))
    }

    /// Try to get as album content.
    pub fn as_album(&self) -> Option<&AlbumContent> {
        match self {
            Self::Album(a) => Some(a),
            _ => None,
        }
    }

    /// Try to get as artist content.
    pub fn as_artist(&self) -> Option<&ArtistContent> {
        match self {
            Self::Artist(a) => Some(a),
            _ => None,
        }
    }

    /// Try to get as playlist content.
    pub fn as_playlist(&self) -> Option<&PlaylistContent> {
        match self {
            Self::Playlist(p) => Some(p),
            _ => None,
        }
    }
}

// =============================================================================
// CONTENT TYPE STRUCTS
// =============================================================================

/// Album content with guaranteed structure.
///
/// Core fields (`id`, `title`, `artist`, `tracks`) are always present.
/// Optional metadata and dynamic extensions vary by backend.
#[derive(Debug, Clone, Default)]
pub struct AlbumContent {
    // === REQUIRED CORE ===
    /// Unique identifier (browseId for YouTube, path for MPD)
    pub id: String,
    /// Album title
    pub title: String,
    /// Primary artist reference
    pub artist: ContentRef,
    /// Album tracks in order
    pub tracks: Vec<Song>,

    // === OPTIONAL METADATA ===
    /// Cover art URL
    pub thumbnail: Option<String>,
    /// Release year
    pub year: Option<u16>,
    /// Album type (Album, Single, EP, Compilation)
    pub release_type: Option<ReleaseType>,
    /// Album description or notes
    pub description: Option<String>,

    // === DYNAMIC EXTENSIONS ===
    /// Optional sections (stats, related content, actions)
    pub extensions: Extensions,
}

/// Artist content with guaranteed structure.
#[derive(Debug, Clone, Default)]
pub struct ArtistContent {
    // === REQUIRED CORE ===
    /// Unique identifier
    pub id: String,
    /// Artist name
    pub name: String,
    /// Top/popular songs
    pub top_songs: Vec<Song>,

    // === OPTIONAL METADATA ===
    /// Profile image URL
    pub thumbnail: Option<String>,
    /// Artist biography
    pub bio: Option<String>,

    // === DYNAMIC EXTENSIONS ===
    /// Optional sections (stats, discography, related artists)
    pub extensions: Extensions,
}

/// Playlist content with guaranteed structure.
#[derive(Debug, Clone, Default)]
pub struct PlaylistContent {
    // === REQUIRED CORE ===
    /// Unique identifier
    pub id: String,
    /// Playlist title
    pub title: String,
    /// Playlist tracks
    pub tracks: Vec<Song>,

    // === OPTIONAL METADATA ===
    /// Playlist author/creator
    pub author: Option<ContentRef>,
    /// Cover art URL
    pub thumbnail: Option<String>,
    /// Playlist description
    pub description: Option<String>,
    /// Total track count (may differ from tracks.len() if paginated)
    pub track_count: Option<usize>,
    /// Total duration as display text
    pub duration_text: Option<String>,

    // === DYNAMIC EXTENSIONS ===
    /// Optional sections (stats, featured artists, related playlists)
    pub extensions: Extensions,
}

// =============================================================================
// CONTENT REFERENCE
// =============================================================================

/// Lightweight reference to content for relationships and navigation.
///
/// Used for artist links, related albums, etc. without full content data.
#[derive(Debug, Clone, Default)]
pub struct ContentRef {
    /// Unique identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Content type for navigation
    pub content_type: ContentType,
    /// Thumbnail URL
    pub thumbnail: Option<String>,
    /// Secondary text (year, track count, author, etc.)
    pub subtitle: Option<String>,
}

impl ContentRef {
    /// Create a new content reference.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            content_type: ContentType::default(),
            thumbnail: None,
            subtitle: None,
        }
    }

    /// Create an artist reference.
    pub fn artist(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            content_type: ContentType::Artist,
            thumbnail: None,
            subtitle: None,
        }
    }

    /// Create an album reference.
    pub fn album(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: title.into(),
            content_type: ContentType::Album,
            thumbnail: None,
            subtitle: None,
        }
    }

    /// Create a playlist reference.
    pub fn playlist(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: title.into(),
            content_type: ContentType::Playlist,
            thumbnail: None,
            subtitle: None,
        }
    }

    /// Add thumbnail.
    pub fn with_thumbnail(mut self, url: impl Into<String>) -> Self {
        self.thumbnail = Some(url.into());
        self
    }

    /// Add subtitle.
    pub fn with_subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }
}

// =============================================================================
// CONTENT TYPE ENUM
// =============================================================================

/// Type of content for navigation and rendering hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ContentType {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
    Directory,
    Video,
}

/// Album release type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseType {
    Album,
    Single,
    EP,
    Compilation,
}

// =============================================================================
// EXTENSIONS SYSTEM
// =============================================================================

/// Container for optional/dynamic sections.
///
/// Backends return only the sections they support. Missing section = feature not supported.
/// This avoids the ambiguity of empty vectors.
#[derive(Debug, Clone, Default)]
pub struct Extensions {
    sections: Vec<Section>,
}

impl Extensions {
    /// Create empty extensions.
    pub fn new() -> Self {
        Self { sections: Vec::new() }
    }

    /// Create a builder for fluent construction.
    pub fn builder() -> ExtensionsBuilder {
        ExtensionsBuilder::new()
    }

    /// Add a section.
    pub fn add(&mut self, section: Section) {
        self.sections.push(section);
    }

    /// Get a section by key.
    pub fn get(&self, key: SectionKey) -> Option<&Section> {
        self.sections.iter().find(|s| s.key == key)
    }

    /// Iterate over all sections.
    pub fn iter(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Get section count.
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Get all stats from the Stats section.
    pub fn stats(&self) -> &[Stat] {
        self.get(SectionKey::Stats)
            .and_then(|s| match &s.content {
                SectionData::Stats(stats) => Some(stats.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    /// Get all actions from the Actions section.
    pub fn actions(&self) -> &[Action] {
        self.get(SectionKey::Actions)
            .and_then(|s| match &s.content {
                SectionData::Actions(actions) => Some(actions.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }
}

/// Builder for ergonomic Extensions construction.
#[derive(Debug, Default)]
pub struct ExtensionsBuilder {
    sections: Vec<Section>,
}

impl ExtensionsBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self { sections: Vec::new() }
    }

    /// Add statistics section.
    pub fn stats(mut self, stats: Vec<Stat>) -> Self {
        if !stats.is_empty() {
            self.sections.push(Section {
                key: SectionKey::Stats,
                title: String::new(),
                content: SectionData::Stats(stats),
            });
        }
        self
    }

    /// Add actions section.
    pub fn actions(mut self, actions: Vec<Action>) -> Self {
        if !actions.is_empty() {
            self.sections.push(Section {
                key: SectionKey::Actions,
                title: String::new(),
                content: SectionData::Actions(actions),
            });
        }
        self
    }

    /// Add related albums section.
    pub fn related_albums(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::RelatedAlbums,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add "more by artist" section.
    pub fn more_by_artist(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::MoreByArtist,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add related artists section.
    pub fn related_artists(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::RelatedArtists,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add albums section (for artist pages).
    pub fn albums(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::Albums,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add singles section (for artist pages).
    pub fn singles(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::Singles,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add featured artists section (for playlists).
    pub fn featured_artists(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::FeaturedArtists,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add related playlists section.
    pub fn related_playlists(mut self, title: impl Into<String>, items: Vec<ContentRef>) -> Self {
        if !items.is_empty() {
            self.sections.push(Section {
                key: SectionKey::RelatedPlaylists,
                title: title.into(),
                content: SectionData::Items(items),
            });
        }
        self
    }

    /// Add a custom section.
    pub fn section(mut self, key: SectionKey, title: impl Into<String>, content: SectionData) -> Self {
        self.sections.push(Section {
            key,
            title: title.into(),
            content,
        });
        self
    }

    /// Build the Extensions container.
    pub fn build(self) -> Extensions {
        Extensions { sections: self.sections }
    }
}

// =============================================================================
// SECTION TYPES
// =============================================================================

/// A section of extended content.
#[derive(Debug, Clone)]
pub struct Section {
    /// Section identifier for lookup and config
    pub key: SectionKey,
    /// Display title (empty = no header)
    pub title: String,
    /// Section content
    pub content: SectionData,
}

/// Well-known section keys (type-safe, extensible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SectionKey {
    // Statistics & Actions
    Stats,
    Actions,

    // Related content
    RelatedAlbums,
    RelatedArtists,
    RelatedPlaylists,
    MoreByArtist,
    FeaturedArtists,

    // Discography (for artist pages)
    Albums,
    Singles,
    Compilations,
    AppearsOn,
}

/// Section data variants (typed, extensible).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SectionData {
    /// List of content references (albums, artists, playlists)
    Items(Vec<ContentRef>),
    /// List of playable songs
    Tracks(Vec<Song>),
    /// Key-value statistics
    Stats(Vec<Stat>),
    /// Available actions
    Actions(Vec<Action>),
    /// Paginated content (for large sections)
    Paginated {
        items: Vec<ContentRef>,
        total: usize,
        has_more: bool,
    },
    /// Error loading this section (partial failure)
    Error(String),
}

// =============================================================================
// STATISTICS
// =============================================================================

/// A statistic to display.
#[derive(Debug, Clone)]
pub struct Stat {
    /// Stat identifier
    pub key: StatKey,
    /// Display label
    pub label: String,
    /// Typed value
    pub value: StatValue,
}

impl Stat {
    /// Create a year stat.
    pub fn year(year: u16) -> Self {
        Self {
            key: StatKey::Year,
            label: "Year".into(),
            value: StatValue::Number(year as i64),
        }
    }

    /// Create a track count stat.
    pub fn track_count(count: usize) -> Self {
        Self {
            key: StatKey::TrackCount,
            label: "Tracks".into(),
            value: StatValue::Number(count as i64),
        }
    }

    /// Create a duration stat.
    pub fn duration(duration: Duration) -> Self {
        Self {
            key: StatKey::Duration,
            label: "Duration".into(),
            value: StatValue::Duration(duration),
        }
    }

    /// Create a subscribers stat.
    pub fn subscribers(text: impl Into<String>) -> Self {
        Self {
            key: StatKey::Subscribers,
            label: "Subscribers".into(),
            value: StatValue::Text(text.into()),
        }
    }

    /// Create a custom text stat.
    pub fn text(key: StatKey, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            value: StatValue::Text(value.into()),
        }
    }
}

/// Stat key for identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StatKey {
    Year,
    TrackCount,
    Duration,
    Subscribers,
    Followers,
    MonthlyListeners,
    Popularity,
    PlayCount,
    Views,
}

/// Typed stat value for proper formatting.
#[derive(Debug, Clone)]
pub enum StatValue {
    /// Text value (displayed as-is)
    Text(String),
    /// Numeric value (UI can format with locale)
    Number(i64),
    /// Duration value (UI can format as "X hr Y min")
    Duration(Duration),
}

impl StatValue {
    /// Get as display string.
    pub fn display(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Duration(d) => {
                let secs = d.as_secs();
                let mins = secs / 60;
                let hours = mins / 60;
                if hours > 0 {
                    format!("{} hr {} min", hours, mins % 60)
                } else {
                    format!("{} min", mins)
                }
            }
        }
    }
}

// =============================================================================
// ACTIONS
// =============================================================================

/// An available action for content.
#[derive(Debug, Clone)]
pub struct Action {
    /// Action type
    pub kind: ActionKind,
    /// Whether action is currently enabled
    pub enabled: bool,
}

impl Action {
    /// Create a Play action.
    pub fn play() -> Self {
        Self { kind: ActionKind::Play, enabled: true }
    }

    /// Create a Shuffle action.
    pub fn shuffle() -> Self {
        Self { kind: ActionKind::Shuffle, enabled: true }
    }

    /// Create a Radio action.
    pub fn radio() -> Self {
        Self { kind: ActionKind::Radio, enabled: true }
    }

    /// Create an AddToQueue action.
    pub fn add_to_queue() -> Self {
        Self { kind: ActionKind::AddToQueue, enabled: true }
    }

    /// Create an AddToLibrary action.
    pub fn add_to_library() -> Self {
        Self { kind: ActionKind::AddToLibrary, enabled: true }
    }

    /// Set enabled state.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Action kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ActionKind {
    Play,
    Shuffle,
    Radio,
    AddToQueue,
    AddToLibrary,
    Share,
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_ref_builders() {
        let artist = ContentRef::artist("id1", "The Beatles")
            .with_thumbnail("https://example.com/beatles.jpg");
        
        assert_eq!(artist.id, "id1");
        assert_eq!(artist.name, "The Beatles");
        assert_eq!(artist.content_type, ContentType::Artist);
        assert!(artist.thumbnail.is_some());
    }

    #[test]
    fn test_extensions_builder() {
        let extensions = Extensions::builder()
            .stats(vec![Stat::year(1969), Stat::track_count(17)])
            .related_albums("More by The Beatles", vec![
                ContentRef::album("album2", "Let It Be"),
            ])
            .actions(vec![Action::play(), Action::shuffle()])
            .build();

        assert_eq!(extensions.len(), 3);
        assert!(extensions.get(SectionKey::Stats).is_some());
        assert!(extensions.get(SectionKey::RelatedAlbums).is_some());
        assert!(extensions.get(SectionKey::Actions).is_some());
        assert!(extensions.get(SectionKey::RelatedArtists).is_none());
    }

    #[test]
    fn test_extensions_accessors() {
        let extensions = Extensions::builder()
            .stats(vec![Stat::year(1969)])
            .actions(vec![Action::play()])
            .build();

        assert_eq!(extensions.stats().len(), 1);
        assert_eq!(extensions.actions().len(), 1);
    }

    #[test]
    fn test_empty_sections_not_added() {
        let extensions = Extensions::builder()
            .stats(vec![])  // Empty - should not be added
            .related_albums("Title", vec![])  // Empty - should not be added
            .build();

        assert!(extensions.is_empty());
    }

    #[test]
    fn test_album_content_default() {
        let album = AlbumContent {
            id: "abc".into(),
            title: "Abbey Road".into(),
            artist: ContentRef::artist("artist1", "The Beatles"),
            tracks: vec![],
            ..Default::default()
        };

        assert_eq!(album.id, "abc");
        assert_eq!(album.title, "Abbey Road");
        assert!(album.thumbnail.is_none());
        assert!(album.extensions.is_empty());
    }

    #[test]
    fn test_content_details_accessors() {
        let album = ContentDetails::Album(AlbumContent {
            id: "abc".into(),
            title: "Abbey Road".into(),
            artist: ContentRef::artist("artist1", "The Beatles"),
            thumbnail: Some("https://example.com/cover.jpg".into()),
            tracks: vec![],
            ..Default::default()
        });

        assert!(album.is_album());
        assert!(!album.is_artist());
        assert_eq!(album.id(), "abc");
        assert_eq!(album.title(), "Abbey Road");
        assert!(album.as_album().is_some());
        assert!(album.as_artist().is_none());
    }

    #[test]
    fn test_stat_value_display() {
        assert_eq!(StatValue::Text("Hello".into()).display(), "Hello");
        assert_eq!(StatValue::Number(1000).display(), "1000");
        assert_eq!(StatValue::Duration(Duration::from_secs(3725)).display(), "1 hr 2 min");
        assert_eq!(StatValue::Duration(Duration::from_secs(300)).display(), "5 min");
    }
}
