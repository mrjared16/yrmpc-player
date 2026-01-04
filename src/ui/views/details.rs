//! View model for detail pages (album, artist, playlist).
//!
//! This module provides the `DetailsPage` view model which transforms
//! domain `ContentDetails` into a structure optimized for UI rendering.
//!
//! # Architecture
//!
//! ```text
//! Domain Layer                    UI Layer
//! ────────────────────────────    ────────────────────────────
//! ContentDetails::Album(...)  →   DetailsPage (view model)
//! ContentDetails::Artist(...) →   ├── header (Artwork, title, stats)
//! ContentDetails::Playlist(.) →   ├── tracks (for body)
//!                                 └── sections (for related content)
//! ```
//!
//! # Example
//!
//! ```ignore
//! // Backend returns domain type
//! let details = backend.details(&item)?;
//!
//! // Convert to view model for rendering
//! let page = DetailsPage::from(details);
//!
//! // Render uniformly
//! render_header(&page);
//! render_tracks(&page.tracks);
//! for section in &page.sections {
//!     render_section(section);
//! }
//! ```

use crate::domain::{
    Action as DomainAction,
    ActionKind,
    AlbumContent,
    ArtistContent,
    ContentDetails,
    ContentRef,
    ContentType,
    Extensions,
    PlaylistContent,
    QueueContent,
    SearchResultsContent,
    Section as DomainSection,
    SectionData,
    SectionKey,
    Song,
    Stat as DomainStat,
    StatValue,
};

/// Visual identity for the content.
#[derive(Debug, Clone, Default)]
pub struct Artwork {
    /// Square cover/avatar image URL.
    pub thumbnail: Option<String>,
    /// Wide banner image URL (for artist pages).
    pub backdrop: Option<String>,
}

/// A single statistic to display.
#[derive(Debug, Clone)]
pub struct Stat {
    pub label: String,
    pub value: String,
}

/// Section of related content.
#[derive(Debug, Clone)]
pub struct Section {
    pub title: String,
    pub items: Vec<SectionItem>,
    pub layout: Layout,
}

/// Item within a section.
#[derive(Debug, Clone)]
pub struct SectionItem {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub item_type: SectionItemType,
}

/// Type of section item for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionItemType {
    Album,
    Artist,
    Playlist,
    Track,
}

/// Layout hint for rendering a section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Layout {
    /// Vertical list (tracks, top songs).
    #[default]
    List,
    /// Grid of cards (albums, playlists).
    Grid,
    /// Horizontal carousel (related artists).
    Carousel,
}

/// Available action for the content.
#[derive(Debug, Clone)]
pub enum Action {
    /// Play all content.
    Play { id: String },
    /// Shuffle play.
    Shuffle { id: String },
    /// Start radio/mix.
    Radio { id: String },
    /// Add to queue.
    AddToQueue { id: String },
}

/// Content type for the details page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Search,
    Album,
    Artist,
    Playlist,
    Queue,
}

/// View model for rendering detail pages.
///
/// This is the main structure used by UI code to render album, artist,
/// or playlist detail pages. It provides a unified structure regardless
/// of the content type.
#[derive(Debug, Clone)]
pub struct DetailsPage {
    /// Content identifier.
    pub id: String,
    /// Type of content for type-specific UI tweaks.
    pub kind: PageKind,
    /// Primary title.
    pub title: String,
    /// Secondary text (artist for album, creator for playlist).
    pub subtitle: Option<String>,
    /// Cover art / profile image.
    pub artwork: Artwork,
    /// Statistics to display.
    pub stats: Vec<Stat>,
    /// Long-form description.
    pub description: Option<String>,
    /// Main playable tracks.
    pub tracks: Vec<Song>,
    /// Related content sections.
    pub sections: Vec<Section>,
    /// Available actions.
    pub actions: Vec<Action>,
}

impl Default for DetailsPage {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: PageKind::Album,
            title: String::new(),
            subtitle: None,
            artwork: Artwork::default(),
            stats: Vec::new(),
            description: None,
            tracks: Vec::new(),
            sections: Vec::new(),
            actions: Vec::new(),
        }
    }
}

impl DetailsPage {
    /// Check if this page has any tracks.
    pub fn has_tracks(&self) -> bool {
        !self.tracks.is_empty()
    }

    /// Check if this page has any related sections.
    pub fn has_sections(&self) -> bool {
        !self.sections.is_empty()
    }

    /// Get track count.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }
}

// =============================================================================
// Conversions from domain types to view model
// =============================================================================

impl From<ContentDetails> for DetailsPage {
    fn from(details: ContentDetails) -> Self {
        match details {
            ContentDetails::Search(search) => search.into(),
            ContentDetails::Album(album) => album.into(),
            ContentDetails::Artist(artist) => artist.into(),
            ContentDetails::Playlist(playlist) => playlist.into(),
            ContentDetails::Queue(queue) => queue.into(),
        }
    }
}

impl From<SearchResultsContent> for DetailsPage {
    fn from(search: SearchResultsContent) -> Self {
        // Search results don't fit the DetailsPage model well,
        // but we provide a minimal conversion for compatibility.
        // In practice, search results should be handled separately
        // via BrowseStack which preserves the DetailItem structure.
        DetailsPage {
            id: search.query.clone(),
            kind: PageKind::Search,
            title: search.title,
            subtitle: None,
            artwork: Artwork::default(),
            stats: vec![],
            description: None,
            tracks: vec![], // Search items are DetailItems, not Songs
            sections: vec![],
            actions: vec![],
        }
    }
}

impl From<AlbumContent> for DetailsPage {
    fn from(album: AlbumContent) -> Self {
        // Convert stats from extensions
        let stats = convert_stats(album.extensions.stats());

        // Convert sections from extensions (filter out Stats and Actions)
        let sections = convert_sections(&album.extensions);

        // Convert actions from extensions
        let actions = convert_actions(album.extensions.actions(), &album.id);

        DetailsPage {
            id: album.id.clone(),
            kind: PageKind::Album,
            title: album.title,
            subtitle: Some(album.artist.name),
            artwork: Artwork { thumbnail: album.thumbnail, backdrop: None },
            stats,
            description: album.description,
            tracks: album.tracks,
            sections,
            actions,
        }
    }
}

impl From<ArtistContent> for DetailsPage {
    fn from(artist: ArtistContent) -> Self {
        // Convert stats from extensions
        let stats = convert_stats(artist.extensions.stats());

        // Convert sections from extensions
        let sections = convert_sections(&artist.extensions);

        // Convert actions from extensions
        let actions = convert_actions(artist.extensions.actions(), &artist.id);

        DetailsPage {
            id: artist.id.clone(),
            kind: PageKind::Artist,
            title: artist.name,
            subtitle: None,
            artwork: Artwork { thumbnail: artist.thumbnail, backdrop: None },
            stats,
            description: artist.bio,
            tracks: artist.top_songs,
            sections,
            actions,
        }
    }
}

impl From<PlaylistContent> for DetailsPage {
    fn from(playlist: PlaylistContent) -> Self {
        // Convert stats from extensions
        let stats = convert_stats(playlist.extensions.stats());

        // Convert sections from extensions
        let sections = convert_sections(&playlist.extensions);

        // Convert actions from extensions
        let actions = convert_actions(playlist.extensions.actions(), &playlist.id);

        DetailsPage {
            id: playlist.id.clone(),
            kind: PageKind::Playlist,
            title: playlist.title,
            subtitle: playlist.author.map(|a| a.name),
            artwork: Artwork { thumbnail: playlist.thumbnail, backdrop: None },
            stats,
            description: playlist.description,
            tracks: playlist.tracks,
            sections,
            actions,
        }
    }
}

impl From<QueueContent> for DetailsPage {
    fn from(queue: QueueContent) -> Self {
        DetailsPage {
            id: "queue".to_string(),
            kind: PageKind::Queue,
            title: queue.title,
            subtitle: None,
            artwork: Artwork::default(),
            stats: vec![Stat { label: "Items".to_string(), value: queue.songs.len().to_string() }],
            description: None,
            tracks: queue.songs,
            sections: vec![],
            actions: vec![],
        }
    }
}

// =============================================================================
// Helper functions for conversion
// =============================================================================

fn convert_stats(domain_stats: &[DomainStat]) -> Vec<Stat> {
    domain_stats
        .iter()
        .map(|s| Stat {
            label: s.label.clone(),
            value: match &s.value {
                StatValue::Text(t) => t.clone(),
                StatValue::Number(n) => n.to_string(),
                StatValue::Duration(d) => format_duration(*d),
            },
        })
        .collect()
}

fn convert_sections(extensions: &Extensions) -> Vec<Section> {
    extensions
        .iter()
        .filter(|s| !matches!(s.key, SectionKey::Stats | SectionKey::Actions))
        .filter_map(|s| {
            match &s.content {
                SectionData::Items(refs) => {
                    let items: Vec<SectionItem> = refs
                        .iter()
                        .map(|r| SectionItem {
                            id: r.id.clone(),
                            title: r.name.clone(),
                            subtitle: r.subtitle.clone(),
                            thumbnail: r.thumbnail.clone(),
                            item_type: content_type_to_section_type(&r.content_type),
                        })
                        .collect();

                    if items.is_empty() {
                        return None;
                    }

                    // Choose layout based on section key
                    let layout = match s.key {
                        SectionKey::RelatedArtists | SectionKey::FeaturedArtists => {
                            Layout::Carousel
                        }
                        SectionKey::Albums
                        | SectionKey::Singles
                        | SectionKey::MoreByArtist
                        | SectionKey::RelatedPlaylists => Layout::Grid,
                        _ => Layout::List,
                    };

                    Some(Section { title: s.title.clone(), items, layout })
                }
                SectionData::Tracks(_songs) => {
                    // For track sections, skip (tracks are in main tracks field)
                    None
                }
                _ => None,
            }
        })
        .collect()
}

fn convert_actions(domain_actions: &[DomainAction], id: &str) -> Vec<Action> {
    domain_actions
        .iter()
        .filter_map(|a| {
            match a.kind {
                ActionKind::Play => Some(Action::Play { id: id.to_string() }),
                ActionKind::Shuffle => Some(Action::Shuffle { id: id.to_string() }),
                ActionKind::Radio => Some(Action::Radio { id: id.to_string() }),
                ActionKind::AddToQueue => Some(Action::AddToQueue { id: id.to_string() }),
                // Skip actions we don't support in UI yet
                ActionKind::AddToLibrary | ActionKind::Share => None,
            }
        })
        .collect()
}

fn content_type_to_section_type(ct: &ContentType) -> SectionItemType {
    match ct {
        ContentType::Album => SectionItemType::Album,
        ContentType::Artist => SectionItemType::Artist,
        ContentType::Playlist => SectionItemType::Playlist,
        _ => SectionItemType::Track,
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;

    if hours > 0 { format!("{} hr {} min", hours, minutes) } else { format!("{} min", minutes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_album_to_details_page() {
        let album = AlbumContent {
            id: "album1".into(),
            title: "Abbey Road".into(),
            artist: ContentRef::artist("artist1", "The Beatles"),
            year: Some(1969),
            description: None,
            thumbnail: Some("https://example.com/cover.jpg".into()),
            tracks: vec![],
            release_type: None,
            extensions: Extensions::new(),
        };

        let page = DetailsPage::from(ContentDetails::Album(album));

        assert_eq!(page.kind, PageKind::Album);
        assert_eq!(page.title, "Abbey Road");
        assert_eq!(page.subtitle, Some("The Beatles".into()));
    }

    #[test]
    fn test_artist_to_details_page() {
        let artist = ArtistContent {
            id: "artist1".into(),
            name: "The Beatles".into(),
            bio: Some("British rock band".into()),
            thumbnail: None,
            top_songs: vec![],
            extensions: Extensions::new(),
        };

        let page = DetailsPage::from(ContentDetails::Artist(artist));

        assert_eq!(page.kind, PageKind::Artist);
        assert_eq!(page.title, "The Beatles");
        assert!(page.subtitle.is_none()); // Artists don't have subtitle
        assert_eq!(page.description, Some("British rock band".into()));
    }

    #[test]
    fn test_playlist_to_details_page() {
        let playlist = PlaylistContent {
            id: "playlist1".into(),
            title: "My Favorites".into(),
            author: Some(ContentRef::new("user1", "John")),
            tracks: vec![],
            thumbnail: None,
            description: None,
            track_count: Some(10),
            duration_text: Some("45 min".into()),
            extensions: Extensions::new(),
        };

        let page = DetailsPage::from(ContentDetails::Playlist(playlist));

        assert_eq!(page.kind, PageKind::Playlist);
        assert_eq!(page.title, "My Favorites");
        assert_eq!(page.subtitle, Some("John".into()));
    }
}
