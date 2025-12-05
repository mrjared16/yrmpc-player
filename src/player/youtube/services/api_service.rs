//! YouTube Music API service - handles all API calls

use anyhow::Result;
use crate::domain::Song;
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

    /// Search for songs
    pub fn search(&self, query: &str) -> Result<Vec<Song>> {
        self.api.search(query)
    }

    /// Browse artist/album/playlist
    pub fn browse(&self, path: &str) -> Result<Vec<Song>> {
        self.api.browse(path)
    }

    /// Get search suggestions (autocomplete)
    pub fn get_suggestions(&self, _query: &str) -> Result<Vec<String>> {
        // TODO: YouTubeApi doesn't have get_search_suggestions yet
        // Return empty for now, can add later
        Ok(vec![])
    }
}
