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

use std::time::Duration;
use crate::domain::{
    ContentDetails, AlbumDetails, ArtistDetails, PlaylistDetails,
    Song,
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
}

/// Content type for the details page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Album,
    Artist,
    Playlist,
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
            ContentDetails::Album(album) => album.into(),
            ContentDetails::Artist(artist) => artist.into(),
            ContentDetails::Playlist(playlist) => playlist.into(),
        }
    }
}

impl From<AlbumDetails> for DetailsPage {
    fn from(album: AlbumDetails) -> Self {
        let mut stats = Vec::new();
        if let Some(year) = &album.year {
            stats.push(Stat { label: "Year".into(), value: year.clone() });
        }
        stats.push(Stat { 
            label: "Tracks".into(), 
            value: format!("{} tracks", album.tracks.len()) 
        });

        let mut sections = Vec::new();
        if !album.more_by_artist.is_empty() {
            sections.push(Section {
                title: "More by artist".into(),
                items: album.more_by_artist.into_iter().map(|a| SectionItem {
                    id: a.id,
                    title: a.title,
                    subtitle: a.year,
                    thumbnail: a.thumbnail,
                    item_type: SectionItemType::Album,
                }).collect(),
                layout: Layout::Grid,
            });
        }

        DetailsPage {
            id: album.id.clone(),
            kind: PageKind::Album,
            title: album.title,
            subtitle: Some(album.artist.name),
            artwork: Artwork {
                thumbnail: album.thumbnail,
                backdrop: None,
            },
            stats,
            description: album.description,
            tracks: album.tracks,
            sections,
            actions: vec![
                Action::Play { id: album.id.clone() },
                Action::Shuffle { id: album.id },
            ],
        }
    }
}

impl From<ArtistDetails> for DetailsPage {
    fn from(artist: ArtistDetails) -> Self {
        let mut stats = Vec::new();
        if let Some(subs) = &artist.subscribers {
            stats.push(Stat { label: "Subscribers".into(), value: subs.clone() });
        }

        let mut sections = Vec::new();
        
        if !artist.albums.is_empty() {
            sections.push(Section {
                title: "Albums".into(),
                items: artist.albums.into_iter().map(|a| SectionItem {
                    id: a.id,
                    title: a.title,
                    subtitle: a.year,
                    thumbnail: a.thumbnail,
                    item_type: SectionItemType::Album,
                }).collect(),
                layout: Layout::Grid,
            });
        }

        if !artist.singles.is_empty() {
            sections.push(Section {
                title: "Singles".into(),
                items: artist.singles.into_iter().map(|a| SectionItem {
                    id: a.id,
                    title: a.title,
                    subtitle: a.year,
                    thumbnail: a.thumbnail,
                    item_type: SectionItemType::Album,
                }).collect(),
                layout: Layout::Grid,
            });
        }

        if !artist.related_artists.is_empty() {
            sections.push(Section {
                title: "Fans also like".into(),
                items: artist.related_artists.into_iter().map(|a| SectionItem {
                    id: a.id,
                    title: a.name,
                    subtitle: None,
                    thumbnail: a.thumbnail,
                    item_type: SectionItemType::Artist,
                }).collect(),
                layout: Layout::Carousel,
            });
        }

        DetailsPage {
            id: artist.id.clone(),
            kind: PageKind::Artist,
            title: artist.name,
            subtitle: None,
            artwork: Artwork {
                thumbnail: artist.thumbnail,
                backdrop: None, // Could add banner support later
            },
            stats,
            description: artist.description,
            tracks: artist.top_songs,
            sections,
            actions: vec![
                Action::Play { id: artist.id.clone() },
                Action::Shuffle { id: artist.id },
            ],
        }
    }
}

impl From<PlaylistDetails> for DetailsPage {
    fn from(playlist: PlaylistDetails) -> Self {
        let mut stats = Vec::new();
        stats.push(Stat { 
            label: "Tracks".into(), 
            value: format!("{} tracks", playlist.track_count) 
        });
        if let Some(duration) = &playlist.duration_text {
            stats.push(Stat { label: "Duration".into(), value: duration.clone() });
        }

        let mut sections = Vec::new();
        
        if !playlist.featured_artists.is_empty() {
            sections.push(Section {
                title: "Featured artists".into(),
                items: playlist.featured_artists.into_iter().map(|a| SectionItem {
                    id: a.id,
                    title: a.name,
                    subtitle: None,
                    thumbnail: a.thumbnail,
                    item_type: SectionItemType::Artist,
                }).collect(),
                layout: Layout::Carousel,
            });
        }

        if !playlist.related_playlists.is_empty() {
            sections.push(Section {
                title: "Similar playlists".into(),
                items: playlist.related_playlists.into_iter().map(|p| SectionItem {
                    id: p.id,
                    title: p.title,
                    subtitle: p.subtitle,
                    thumbnail: p.thumbnail,
                    item_type: SectionItemType::Playlist,
                }).collect(),
                layout: Layout::Grid,
            });
        }

        DetailsPage {
            id: playlist.id.clone(),
            kind: PageKind::Playlist,
            title: playlist.title,
            subtitle: playlist.author,
            artwork: Artwork {
                thumbnail: playlist.thumbnail,
                backdrop: None,
            },
            stats,
            description: playlist.description,
            tracks: playlist.tracks,
            sections,
            actions: vec![
                Action::Play { id: playlist.id.clone() },
                Action::Shuffle { id: playlist.id },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ArtistRef, AlbumRef};

    #[test]
    fn test_album_to_details_page() {
        let album = AlbumDetails {
            id: "album1".into(),
            title: "Abbey Road".into(),
            artist: ArtistRef::new("artist1", "The Beatles"),
            year: Some("1969".into()),
            description: None,
            thumbnail: Some("https://example.com/cover.jpg".into()),
            tracks: vec![],
            more_by_artist: vec![
                AlbumRef::new("album2", "Let It Be"),
            ],
        };

        let page = DetailsPage::from(ContentDetails::Album(album));

        assert_eq!(page.kind, PageKind::Album);
        assert_eq!(page.title, "Abbey Road");
        assert_eq!(page.subtitle, Some("The Beatles".into()));
        assert_eq!(page.stats.len(), 2); // Year + Tracks
        assert_eq!(page.sections.len(), 1); // More by artist
    }

    #[test]
    fn test_artist_to_details_page() {
        let artist = ArtistDetails {
            id: "artist1".into(),
            name: "The Beatles".into(),
            subscribers: Some("10M subscribers".into()),
            description: Some("British rock band".into()),
            thumbnail: None,
            top_songs: vec![],
            albums: vec![],
            singles: vec![],
            related_artists: vec![],
        };

        let page = DetailsPage::from(ContentDetails::Artist(artist));

        assert_eq!(page.kind, PageKind::Artist);
        assert_eq!(page.title, "The Beatles");
        assert!(page.subtitle.is_none()); // Artists don't have subtitle
        assert_eq!(page.stats.len(), 1); // Subscribers
        assert_eq!(page.description, Some("British rock band".into()));
    }
}
