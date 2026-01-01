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
use super::DetailItem;

// =============================================================================
// CONTENT DETAILS ENUM
// =============================================================================

/// Type-safe content details with guaranteed structure per type.
///
/// Each variant has required core fields plus optional extensions.
/// UI code can pattern match for type-specific rendering.
#[derive(Debug, Clone)]
pub enum ContentDetails {
    /// Search results with configurable section ordering.
    Search(SearchResultsContent),
    Album(AlbumContent),
    Artist(ArtistContent),
    Playlist(PlaylistContent),
    /// Play queue (flat list of songs).
    Queue(QueueContent),
}

impl ContentDetails {
    /// Get the content ID regardless of type.
    pub fn id(&self) -> &str {
        match self {
            Self::Search(s) => &s.query,
            Self::Album(a) => &a.id,
            Self::Artist(a) => &a.id,
            Self::Playlist(p) => &p.id,
            Self::Queue(_) => "queue",
        }
    }

    /// Get the title regardless of type.
    pub fn title(&self) -> &str {
        match self {
            Self::Search(s) => &s.title,
            Self::Album(a) => &a.title,
            Self::Artist(a) => &a.name,
            Self::Playlist(p) => &p.title,
            Self::Queue(q) => &q.title,
        }
    }

    /// Get the thumbnail URL if available.
    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Search(_) => None,
            Self::Album(a) => a.thumbnail.as_deref(),
            Self::Artist(a) => a.thumbnail.as_deref(),
            Self::Playlist(p) => p.thumbnail.as_deref(),
            Self::Queue(_) => None,
        }
    }

    /// Get the primary tracks/songs.
    pub fn tracks(&self) -> &[Song] {
        match self {
            Self::Search(_) => &[], // Search results are DetailItems, not Songs
            Self::Album(a) => &a.tracks,
            Self::Artist(a) => &a.top_songs,
            Self::Playlist(p) => &p.tracks,
            Self::Queue(q) => &q.songs,
        }
    }

    /// Get the extensions container.
    pub fn extensions(&self) -> &Extensions {
        static EMPTY_EXTENSIONS: std::sync::LazyLock<Extensions> = 
            std::sync::LazyLock::new(Extensions::new);
        match self {
            Self::Search(_) => &EMPTY_EXTENSIONS,
            Self::Album(a) => &a.extensions,
            Self::Artist(a) => &a.extensions,
            Self::Playlist(p) => &p.extensions,
            Self::Queue(_) => &EMPTY_EXTENSIONS,
        }
    }

    /// Check if this is search results.
    pub fn is_search(&self) -> bool {
        matches!(self, Self::Search(_))
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

    /// Check if this is a queue.
    pub fn is_queue(&self) -> bool {
        matches!(self, Self::Queue(_))
    }

    /// Try to get as search results.
    pub fn as_search(&self) -> Option<&SearchResultsContent> {
        match self {
            Self::Search(s) => Some(s),
            _ => None,
        }
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

    /// Try to get as queue content.
    pub fn as_queue(&self) -> Option<&QueueContent> {
        match self {
            Self::Queue(q) => Some(q),
            _ => None,
        }
    }

    /// Convert content details into domain sections.
    /// This is the domain-side of the adapter pattern.
    pub fn into_sections(&self) -> Vec<Section> {
        let mut sections = Vec::new();

        match self {
            Self::Search(search) => {
                // Search: all items in one section (headers are inline)
                if !search.items.is_empty() {
                    sections.push(Section {
                        key: SectionKey::SearchResults,
                        title: String::new(),
                        content: SectionData::Items(
                            search.items.iter()
                                .filter_map(|item| item.as_content_ref().cloned())
                                .collect()
                        ),
                    });
                }
            }

            Self::Album(album) => {
                // Album: tracks section
                if !album.tracks.is_empty() {
                    sections.push(Section {
                        key: SectionKey::Tracks,
                        title: "Tracks".to_string(),
                        content: SectionData::Tracks(album.tracks.clone()),
                    });
                }
                // Add extension sections
                sections.extend(album.extensions.sections.clone());
            }

            Self::Artist(artist) => {
                // Artist: top songs section
                if !artist.top_songs.is_empty() {
                    sections.push(Section {
                        key: SectionKey::TopSongs,
                        title: "Top Songs".to_string(),
                        content: SectionData::Tracks(artist.top_songs.clone()),
                    });
                }
                // Add extension sections
                sections.extend(artist.extensions.sections.clone());
            }

            Self::Playlist(playlist) => {
                // Playlist: tracks section
                if !playlist.tracks.is_empty() {
                    sections.push(Section {
                        key: SectionKey::Tracks,
                        title: "Tracks".to_string(),
                        content: SectionData::Tracks(playlist.tracks.clone()),
                    });
                }
                // Add extension sections
                sections.extend(playlist.extensions.sections.clone());
            }

            Self::Queue(queue) => {
                // Queue: optionally split into Now Playing and Up Next
                if !queue.songs.is_empty() {
                    if let Some(current_idx) = queue.current_index {
                        if current_idx < queue.songs.len() {
                            // Now Playing
                            sections.push(Section {
                                key: SectionKey::NowPlaying,
                                title: "Now Playing".to_string(),
                                content: SectionData::Tracks(vec![queue.songs[current_idx].clone()]),
                            });

                            // Up Next
                            let up_next: Vec<Song> = queue.songs
                                .iter()
                                .skip(current_idx + 1)
                                .cloned()
                                .collect();
                            if !up_next.is_empty() {
                                sections.push(Section {
                                    key: SectionKey::UpNext,
                                    title: "Up Next".to_string(),
                                    content: SectionData::Tracks(up_next),
                                });
                            }
                        }
                    } else {
                        // No current song, flat list
                        sections.push(Section {
                            key: SectionKey::Tracks,
                            title: String::new(),
                            content: SectionData::Tracks(queue.songs.clone()),
                        });
                    }
                }
            }
        }

        sections
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

/// Search results content with items grouped by type.
/// 
/// Unlike other content types, search results are already `DetailItem`s
/// (songs, albums, artists, playlists mixed together with headers).
/// Section ordering is controlled by `config.search.sections`.
#[derive(Debug, Clone, Default)]
pub struct SearchResultsContent {
    /// The search query that produced these results
    pub query: String,
    /// Display title (usually "Results" or the query)
    pub title: String,
    /// All search result items (songs, albums, artists, playlists with headers)
    pub items: Vec<DetailItem>,
}

/// Queue content for displaying the play queue in a ContentView.
///
/// Provides a ContentViewable wrapper around a song list for unified
/// handling by ContentView. Includes current_index for "Now Playing" section.
#[derive(Debug, Clone, Default)]
pub struct QueueContent {
    /// Songs in the queue
    pub songs: Vec<Song>,
    /// Display title
    pub title: String,
    /// Index of currently playing song (if any)
    pub current_index: Option<usize>,
}

impl SearchResultsContent {
    /// Create new search results.
    pub fn new(query: impl Into<String>, items: Vec<DetailItem>) -> Self {
        let query = query.into();
        let title = if query.is_empty() { 
            "Results".to_string() 
        } else { 
            format!("\"{}\"", query)
        };
        Self { query, title, items }
    }
}

impl QueueContent {
    /// Create new queue content.
    pub fn new(songs: Vec<Song>, current_index: Option<usize>) -> Self {
        let count = songs.len();
        Self {
            songs,
            title: format!("Queue ({} items)", count),
            current_index,
        }
    }

    /// Create with a custom title.
    pub fn with_title(songs: Vec<Song>, title: impl Into<String>, current_index: Option<usize>) -> Self {
        Self {
            songs,
            title: title.into(),
            current_index,
        }
    }
}

// =============================================================================
// SEARCHABLE CONTENT (for SearchPane heterogeneous stacking)
// =============================================================================

/// Content types that can appear in SearchPane's navigation stack.
///
/// This enum wraps all content types that SearchPane can display,
/// allowing ContentView<SearchableContent> to handle heterogeneous
/// content while maintaining type safety.
#[derive(Debug, Clone)]
pub enum SearchableContent {
    /// Initial search results
    Results(SearchResultsContent),
    /// Album detail view
    Album(AlbumContent),
    /// Artist detail view
    Artist(ArtistContent),
    /// Playlist detail view
    Playlist(PlaylistContent),
}

impl SearchableContent {
    /// Create from search results.
    pub fn results(query: impl Into<String>, items: Vec<DetailItem>) -> Self {
        Self::Results(SearchResultsContent::new(query, items))
    }

    /// Create from album content.
    pub fn album(content: AlbumContent) -> Self {
        Self::Album(content)
    }

    /// Create from artist content.
    pub fn artist(content: ArtistContent) -> Self {
        Self::Artist(content)
    }

    /// Create from playlist content.
    pub fn playlist(content: PlaylistContent) -> Self {
        Self::Playlist(content)
    }
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

// ListItemDisplay implementation for ContentRef
impl super::display::ListItemDisplay for ContentRef {
    fn primary_text(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.name)
    }

    fn secondary_text(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.subtitle.as_ref().map(|s| std::borrow::Cow::Borrowed(s.as_str()))
    }

    fn thumbnail_url(&self) -> Option<&str> {
        self.thumbnail.as_deref()
    }

    fn type_icon(&self) -> &str {
        match self.content_type {
            ContentType::Artist => " ",  // Nerd Font artist icon
            ContentType::Album => " ",   // Nerd Font disc icon
            ContentType::Playlist => " ", // Nerd Font list icon
            ContentType::Directory => " ", // Nerd Font folder icon
            ContentType::Video => " ",   // Nerd Font video icon
            ContentType::Track => " ",   // Nerd Font music icon
            ContentType::Header => "─",  // Section divider
        }
    }

    fn is_focusable(&self) -> bool {
        true
    }
}

// =============================================================================
// CONTENT TYPE ENUM
// =============================================================================

/// Type of content for navigation and rendering hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]  // Allow adding variants without breaking changes
pub enum ContentType {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
    Directory,
    Video,
    /// Section header in search results (merged from api::ContentType)
    Header,
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

    // Content sections
    Tracks,         // Album/playlist tracks
    TopSongs,       // Artist top songs
    SearchResults,  // Search result items (legacy)
    NowPlaying,     // Queue current track
    UpNext,         // Queue upcoming tracks

    // Search result sections (grouped by content type)
    Songs,          // Search: song results
    Artists,        // Search: artist results
    Playlists,      // Search: playlist results
    Videos,         // Search: video results

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

// =============================================================================
// CONTENT TRAIT (For ContentView<C>)
// =============================================================================

/// Trait for content that can be displayed in a ContentView.
///
/// Implement this trait for each content type (Artist, Album, Playlist, etc.)
/// to enable unified handling by ContentView.
///
/// This trait lives in domain to avoid circular dependencies between
/// domain::content and ui::widgets::content_view.
pub trait ContentViewable: Clone + std::fmt::Debug + Send + 'static {
    /// Get the title for display (breadcrumb, header)
    fn title(&self) -> &str;

    /// Get unique identifier
    fn content_id(&self) -> &str;

    /// Convert to ContentDetails for section building
    fn to_content_details(&self) -> ContentDetails;

    /// Get content as sections (adapter pattern).
    /// Domain provides structured sections, UI converts to SectionView.
    fn sections(&self) -> Vec<Section> {
        // Default implementation converts to ContentDetails and extracts sections
        // Override for custom section layouts
        self.to_content_details().into_sections()
    }
}

impl ContentViewable for AlbumContent {
    fn title(&self) -> &str {
        &self.title
    }

    fn content_id(&self) -> &str {
        &self.id
    }

    fn to_content_details(&self) -> ContentDetails {
        ContentDetails::Album(self.clone())
    }
}

impl ContentViewable for ArtistContent {
    fn title(&self) -> &str {
        &self.name
    }

    fn content_id(&self) -> &str {
        &self.id
    }

    fn to_content_details(&self) -> ContentDetails {
        ContentDetails::Artist(self.clone())
    }
}

impl ContentViewable for PlaylistContent {
    fn title(&self) -> &str {
        &self.title
    }

    fn content_id(&self) -> &str {
        &self.id
    }

    fn to_content_details(&self) -> ContentDetails {
        ContentDetails::Playlist(self.clone())
    }
}

impl ContentViewable for SearchResultsContent {
    fn title(&self) -> &str {
        &self.title
    }

    fn content_id(&self) -> &str {
        &self.query
    }

    fn to_content_details(&self) -> ContentDetails {
        ContentDetails::Search(self.clone())
    }
}

impl ContentViewable for QueueContent {
    fn title(&self) -> &str {
        &self.title
    }

    fn content_id(&self) -> &str {
        "queue"
    }

    fn to_content_details(&self) -> ContentDetails {
        ContentDetails::Queue(self.clone())
    }
}

impl ContentViewable for SearchableContent {
    fn title(&self) -> &str {
        match self {
            Self::Results(c) => c.title(),
            Self::Album(c) => c.title(),
            Self::Artist(c) => c.title(),
            Self::Playlist(c) => c.title(),
        }
    }

    fn content_id(&self) -> &str {
        match self {
            Self::Results(c) => c.content_id(),
            Self::Album(c) => c.content_id(),
            Self::Artist(c) => c.content_id(),
            Self::Playlist(c) => c.content_id(),
        }
    }

    fn to_content_details(&self) -> ContentDetails {
        match self {
            Self::Results(c) => c.to_content_details(),
            Self::Album(c) => c.to_content_details(),
            Self::Artist(c) => c.to_content_details(),
            Self::Playlist(c) => c.to_content_details(),
        }
    }
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
