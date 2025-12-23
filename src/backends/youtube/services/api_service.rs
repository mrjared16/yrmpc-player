//! YouTube Music API service - handles all API calls

use anyhow::Result;
use crate::domain::Song;
use crate::domain::search::SearchItem;
use crate::backends::youtube::details::{PlaylistDetails, AlbumDetails, ArtistDetails};
use super::super::api::YouTubeApi;

/// API service manages YouTube Music API interactions
pub struct ApiService {
    api: YouTubeApi,
}

impl ApiService {
    /// Create new API service
    pub fn new() -> Result<Self> {
        Ok(Self {
            api: YouTubeApi::new()?,
        })
    }

    /// Load cookies from file
    pub fn load_cookies(&self, path: &str) -> Result<()> {
        self.api.load_cookies(path)
    }

    /// Search for music - returns type-safe SearchItem enum
    pub fn search_items(&self, query: &str) -> Result<Vec<SearchItem>> {
        self.api.search_items(query)
    }

    /// Browse artist/album/playlist
    pub fn browse(&self, path: &str) -> Result<Vec<Song>> {
        self.api.browse(path)
    }

    /// Get search suggestions (autocomplete)
    pub fn get_suggestions(&self, query: &str) -> Result<Vec<String>> {
        self.api.get_suggestions(query)
    }
    
    /// Get detailed playlist info with tracks and metadata
    pub fn get_playlist_details(&self, playlist_id: &str) -> Result<PlaylistDetails> {
        self.api.get_playlist_details(playlist_id)
    }
    
    /// Get detailed album info with tracks and metadata
    pub fn get_album_details(&self, album_id: &str) -> Result<AlbumDetails> {
        self.api.get_album_details(album_id)
    }
    
    /// Get detailed artist info with discography
    pub fn get_artist_details(&self, artist_id: &str) -> Result<ArtistDetails> {
        self.api.get_artist_details(artist_id)
    }
}
