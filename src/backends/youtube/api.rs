//! YouTube Music API wrapper.
//! Separated for independent testing - can mock API responses.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use parking_lot::Mutex;
use tokio::runtime::Runtime;
use ytmapi_rs::{
    YtMusic,
    auth::BrowserToken,
    common::{AlbumID, ArtistChannelID, PlaylistID, VideoID, YoutubeID},
    parse::{BasicSearchResultCommunityPlaylist, SearchResultVideo},
    query::{
        GetAlbumQuery, GetArtistQuery, GetLibraryAlbumsQuery, GetLibraryArtistsQuery,
        GetLibraryPlaylistsQuery, GetLibrarySongsQuery, GetWatchPlaylistQuery, SearchQuery,
        GetSearchSuggestionsQuery, GetPlaylistDetailsQuery,
    },
};

use super::protocol::SongData;
use crate::domain::Song;

/// YouTube Music API client
pub struct YouTubeApi {
    rt: Runtime,
    api: Arc<Mutex<Option<YtMusic<BrowserToken>>>>,
}

impl YouTubeApi {
    pub fn new() -> Result<Self> {
        let rt = Runtime::new().context("Failed to create Tokio runtime")?;
        Ok(Self {
            rt,
            api: Arc::new(Mutex::new(None)),
        })
    }

    /// Load cookies from file to authenticate API
    pub fn load_cookies(&self, path: &str) -> Result<()> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read cookie file: {}", path))?;

        // Parse Netscape format cookies
        let mut cookie_parts = Vec::new();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 7 {
                cookie_parts.push(format!("{}={}", parts[5], parts[6]));
            }
        }

        if cookie_parts.is_empty() {
            return Err(anyhow!("No valid cookies found in file"));
        }

        let cookie_string = cookie_parts.join("; ");
        log::debug!("Parsed {} cookies", cookie_parts.len());

        let api = self.rt.block_on(async {
            use ytmapi_rs::{Client, YtMusicBuilder};
            let client = Client::new()?;
            let token = BrowserToken::from_str(&cookie_string, &client).await?;
            YtMusicBuilder::new_with_client(client).with_browser_token(token).build()
        })?;

        *self.api.lock() = Some(api);
        log::info!("YouTube API initialized with cookies");
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.api.lock().is_some()
    }

    /// Sanitize user-provided search queries to avoid invalid arguments to the
    /// YouTube Music API (which can cause HTTP 400 errors).
    fn sanitize_query(raw: &str) -> Option<String> {
        let cleaned: String = raw
            .trim()
            .chars()
            .filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
            .collect();
        let cleaned = cleaned.trim();
        if cleaned.is_empty() { None } else { Some(cleaned.to_string()) }
    }


    /// Search for music - returns type-safe SearchItem enum
    /// 
    /// This is the new recommended search method that uses the domain::search types
    /// for exhaustive type matching and proper separation of playable vs browsable items.
    pub fn search_items(&self, query: &str) -> Result<Vec<crate::domain::search::SearchItem>> {
        use crate::domain::search::{SearchItem, PlayableItem, BrowsableItem, SongItem, VideoItem, ArtistItem, AlbumItem, PlaylistItem};
        
        let raw_query = query;
        let query = match Self::sanitize_query(raw_query) {
            Some(q) => q,
            None => {
                log::debug!("YouTube API: search_items called with empty/invalid query, skipping");
                return Ok(Vec::new());
            }
        };

        log::debug!("YouTube API: search_items(query='{}', raw='{}')", query, raw_query);
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;

        let query_for_log = query.clone();
        let results = self
            .rt
            .block_on(async move {
                let search_query = SearchQuery::new(query);
                api.query(search_query).await
            })
            .map_err(|e| {
                log::error!("YouTube API search_items failed for query='{}': {}", query_for_log, e);
                e
            })?;

        let mut items = Vec::new();

        // Top results
        if !results.top_results.is_empty() {
            log::info!("search_items: {} top_results found for '{}'", results.top_results.len(), query_for_log);
            items.push(SearchItem::Header("Top Result".into()));
            for (idx, r) in results.top_results.into_iter().enumerate() {
                log::debug!("  TopResult[{}]: name='{}', type={:?}, video_id={:?}, browse_id={:?}, byline={:?}, artist={:?}", 
                    idx, r.result_name, r.result_type, r.video_id, r.browse_id, r.byline, r.artist);
                match SearchItem::try_from(r) {
                    Ok(item) => items.push(item),
                    Err(e) => log::warn!("  TopResult[{}] conversion failed: {}", idx, e),
                }
            }
        } else {
            log::warn!("search_items: No top_results from API for '{}'", query_for_log);
        }

        // Artists
        if !results.artists.is_empty() {
            items.push(SearchItem::Header("Artists".into()));
            for a in results.artists {
                items.push(SearchItem::from(a));
            }
        }

        // Albums
        if !results.albums.is_empty() {
            items.push(SearchItem::Header("Albums".into()));
            for a in results.albums {
                items.push(SearchItem::from(a));
            }
        }

        // Songs
        if !results.songs.is_empty() {
            items.push(SearchItem::Header("Songs".into()));
            for s in results.songs {
                items.push(SearchItem::from(s));
            }
        }

        // Videos
        if !results.videos.is_empty() {
            items.push(SearchItem::Header("Videos".into()));
            for v in results.videos {
                if let Ok(item) = SearchItem::try_from(v) {
                    items.push(item);
                }
            }
        }

        // Featured playlists (curated by YouTube Music)
        if !results.featured_playlists.is_empty() {
            items.push(SearchItem::Header("Featured Playlists".into()));
            for p in results.featured_playlists {
                items.push(SearchItem::from(p));
            }
        }

        // Community playlists (user-created)
        if !results.community_playlists.is_empty() {
            items.push(SearchItem::Header("Playlists".into()));
            for p in results.community_playlists {
                items.push(SearchItem::from(p));
            }
        }

        log::info!("search_items returned {} items for '{}'", items.len(), query_for_log);
        Ok(items)
    }

    /// Get search suggestions for autocomplete
    pub fn get_suggestions(&self, query: &str) -> Result<Vec<String>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        log::debug!("YouTube API: get_suggestions(query='{}')", query);
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;

        let query_str = query.to_string();
        let suggestions = self
            .rt
            .block_on(async move {
                let suggestion_query = GetSearchSuggestionsQuery::new(query_str);
                api.query(suggestion_query).await
            })
            .map_err(|e| {
                log::error!("YouTube API get_suggestions failed: {}", e);
                e
            })?;

        // Extract suggestion strings
        let result: Vec<String> = suggestions
            .into_iter()
            .map(|s| s.get_text())
            .collect();

        log::debug!("get_suggestions returned {} suggestions", result.len());
        Ok(result)
    }

    /// Browse an album, artist, or playlist
    pub fn browse(&self, path: &str) -> Result<Vec<Song>> {
        log::debug!("YouTube API: browse(path='{}')", path);
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;

        if let Some(album_id) = path.strip_prefix("album:") {
            return self.browse_album(&api, album_id);
        }
        if let Some(artist_id) = path.strip_prefix("artist:") {
            return self.browse_artist(&api, artist_id);
        }
        if let Some(playlist_id) = path.strip_prefix("playlist:") {
            return self.browse_playlist(&api, playlist_id);
        }

        log::warn!("YouTube API: browse called with unknown path format: '{}'", path);
        Ok(vec![])
    }

    fn browse_album(&self, api: &YtMusic<BrowserToken>, album_id: &str) -> Result<Vec<Song>> {
        log::debug!("YouTube API: browse_album(album_id='{}')", album_id);
        let query = GetAlbumQuery::new(AlbumID::from_raw(album_id));
        let album = self.rt.block_on(api.query(query)).map_err(|e| {
            log::error!("YouTube API browse_album failed for album_id='{}': {}", album_id, e);
            e
        })?;

        let mut songs = Vec::new();
        for track in album.tracks {
            let mut s = Song::default();
            s.uri = track.video_id.get_raw().to_string();
            s.metadata.insert("title".into(), vec![track.title]);
            s.metadata.insert("album".into(), vec![album.title.clone()]);
            if let Some(artist) = album.artists.first() {
                s.metadata.insert("artist".into(), vec![artist.name.clone()]);
            }
            if let Some(thumb) = album.thumbnails.last() {
                s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
            }
            s.duration = Self::parse_duration(&track.duration);
            songs.push(s);
        }
        Ok(songs)
    }

    fn browse_artist(&self, api: &YtMusic<BrowserToken>, artist_id: &str) -> Result<Vec<Song>> {
        log::debug!("YouTube API: browse_artist(artist_id='{}')", artist_id);
        let query = GetArtistQuery::new(ArtistChannelID::from_raw(artist_id));
        let artist = self.rt.block_on(api.query(query)).map_err(|e| {
            log::error!("YouTube API browse_artist failed for artist_id='{}': {}", artist_id, e);
            e
        })?;

        let mut songs = Vec::new();

        // Top songs
        if let Some(top_songs) = artist.top_releases.songs {
            for song in top_songs.results {
                let mut s = Song::default();
                s.uri = song.video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![song.title]);
                s.metadata.insert("artist".into(), vec![artist.name.clone()]);
                s.metadata.insert("album".into(), vec![song.album.name]);
                songs.push(s);
            }
        }

        // Albums as directories
        if let Some(albums) = artist.top_releases.albums {
            for album in albums.results {
                let mut s = Song::default();
                s.uri = format!("album:{}", album.album_id.get_raw());
                s.metadata.insert("title".into(), vec![album.title]);
                s.metadata.insert("type".into(), vec!["album".into()]);
                songs.push(s);
            }
        }

        Ok(songs)
    }

    fn browse_playlist(&self, api: &YtMusic<BrowserToken>, playlist_id: &str) -> Result<Vec<Song>> {
        log::debug!("YouTube API: browse_playlist(playlist_id='{}')", playlist_id);
        let query = GetWatchPlaylistQuery::new_from_playlist_id(PlaylistID::from_raw(playlist_id));
        let playlist = self.rt.block_on(api.query(query)).map_err(|e| {
            log::error!("YouTube API browse_playlist failed for playlist_id='{}': {}", playlist_id, e);
            e
        })?;

        let mut songs = Vec::new();
        for track in playlist {
            let mut s = Song::default();
            s.uri = track.video_id.get_raw().to_string();
            s.metadata.insert("title".into(), vec![track.title]);
            s.metadata.insert("artist".into(), vec![track.author]);
            if let Some(thumb) = track.thumbnails.last() {
                s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
            }
            s.duration = Self::parse_duration(&track.duration);
            songs.push(s);
        }
        Ok(songs)
    }

    // =========================================================================
    // RICH DETAIL METHODS
    // =========================================================================
    // These return structured detail types with metadata and related content
    
    /// Get detailed playlist info with tracks, metadata, and related content
    pub fn get_playlist_details(&self, playlist_id: &str) -> Result<super::details::PlaylistDetails> {
        use super::details::{PlaylistDetails, ArtistRef, PlaylistRef};
        
        let api = self.api.lock();
        let api = api.as_ref().ok_or_else(|| anyhow!("API not authenticated"))?;
        
        log::debug!("YouTube API: get_playlist_details(playlist_id='{}')", playlist_id);
        
        // First, get playlist metadata (title, description, author, etc.)
        let details_query = GetPlaylistDetailsQuery::new(PlaylistID::from_raw(playlist_id));
        let details = self.rt.block_on(api.query(details_query))?;
        
        // Then, get the tracks from watch playlist query
        let tracks_query = GetWatchPlaylistQuery::new_from_playlist_id(PlaylistID::from_raw(playlist_id));
        let playlist_tracks = self.rt.block_on(api.query(tracks_query))?;
        
        let mut tracks = Vec::new();
        for track in playlist_tracks {
            let mut s = Song::default();
            s.uri = track.video_id.get_raw().to_string();
            s.metadata.insert("title".into(), vec![track.title]);
            s.metadata.insert("artist".into(), vec![track.author.clone()]);
            if let Some(thumb) = track.thumbnails.last() {
                s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
            }
            s.duration = Self::parse_duration(&track.duration);
            tracks.push(s);
        }
        
        // Get thumbnail from playlist metadata, fallback to first track
        let thumbnail = details.thumbnails.last()
            .map(|t| t.url.clone())
            .or_else(|| tracks.first().and_then(|t| t.metadata.get("thumbnail").and_then(|v| v.first()).cloned()));
        
        Ok(PlaylistDetails {
            id: playlist_id.to_string(),
            title: details.title,
            artist: Some(details.author),
            year: if details.year.is_empty() { None } else { Some(details.year) },
            thumbnail,
            track_count: tracks.len(),
            duration_text: if details.duration.is_empty() { None } else { Some(details.duration) },
            tracks,
            featured_artists: vec![],
            related_playlists: vec![],
        })
    }
    
    /// Get detailed album info with tracks, metadata, and more from artist
    pub fn get_album_details(&self, album_id: &str) -> Result<super::details::AlbumDetails> {
        use super::details::{AlbumDetails, ArtistRef, AlbumRef};
        
        let api = self.api.lock();
        let api = api.as_ref().ok_or_else(|| anyhow!("API not authenticated"))?;
        
        log::debug!("YouTube API: get_album_details(album_id='{}')", album_id);
        let query = GetAlbumQuery::new(AlbumID::from_raw(album_id));
        let album = self.rt.block_on(api.query(query))?;
        
        let mut tracks = Vec::new();
        for track in album.tracks {
            let mut s = Song::default();
            s.uri = track.video_id.get_raw().to_string();
            s.metadata.insert("title".into(), vec![track.title]);
            s.metadata.insert("album".into(), vec![album.title.clone()]);
            if let Some(artist) = album.artists.first() {
                s.metadata.insert("artist".into(), vec![artist.name.clone()]);
            }
            if let Some(thumb) = album.thumbnails.last() {
                s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
            }
            s.duration = Self::parse_duration(&track.duration);
            tracks.push(s);
        }
        
        // Get artist info - use .id field if available, otherwise use name as fallback
        let artist = album.artists.first().map(|a| ArtistRef {
            id: a.id.as_ref().map(|id| id.get_raw().to_string()).unwrap_or_default(),
            name: a.name.clone(),
            thumbnail: None,
        }).unwrap_or(ArtistRef {
            id: String::new(),
            name: "Unknown Artist".to_string(),
            thumbnail: None,
        });
        
        Ok(AlbumDetails {
            id: album_id.to_string(),
            title: album.title,
            artist,
            year: Some(album.year),
            thumbnail: album.thumbnails.last().map(|t| t.url.clone()),
            tracks,
            more_by_artist: vec![], // Would need additional API call
        })
    }
    
    /// Get detailed artist info with top songs, albums, and related artists
    pub fn get_artist_details(&self, artist_id: &str) -> Result<super::details::ArtistDetails> {
        use super::details::{ArtistDetails, ArtistRef, AlbumRef};
        
        let api = self.api.lock();
        let api = api.as_ref().ok_or_else(|| anyhow!("API not authenticated"))?;
        
        log::debug!("YouTube API: get_artist_details(artist_id='{}')", artist_id);
        let query = GetArtistQuery::new(ArtistChannelID::from_raw(artist_id));
        let artist = self.rt.block_on(api.query(query))?;
        
        let mut top_songs = Vec::new();
        if let Some(songs) = artist.top_releases.songs {
            for song in songs.results {
                let mut s = Song::default();
                s.uri = song.video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![song.title]);
                s.metadata.insert("artist".into(), vec![artist.name.clone()]);
                s.metadata.insert("album".into(), vec![song.album.name]);
                top_songs.push(s);
            }
        }
        
        let albums: Vec<AlbumRef> = artist.top_releases.albums.map(|a| {
            a.results.into_iter().map(|album| AlbumRef {
                id: album.album_id.get_raw().to_string(),
                title: album.title,
                year: Some(album.year),
                thumbnail: album.thumbnails.last().map(|t| t.url.clone()),
            }).collect()
        }).unwrap_or_default();
        
        let singles: Vec<AlbumRef> = artist.top_releases.singles.map(|s| {
            s.results.into_iter().map(|single| AlbumRef {
                id: single.album_id.get_raw().to_string(),
                title: single.title,
                year: Some(single.year),
                thumbnail: single.thumbnails.last().map(|t| t.url.clone()),
            }).collect()
        }).unwrap_or_default();
        
        // Note: related_artists field may not exist in this version of ytmapi_rs
        // Use empty vec as fallback
        let related_artists: Vec<ArtistRef> = vec![];
        
        Ok(ArtistDetails {
            id: artist_id.to_string(),
            name: artist.name,
            subscribers: None, // Field may not exist
            description: artist.description,
            thumbnail: artist.thumbnails.last().map(|t| t.url.clone()),
            top_songs,
            albums,
            singles,
            related_artists,
        })
    }

    fn parse_top_result(&self, r: ytmapi_rs::parse::TopResult) -> Option<Song> {
        use ytmapi_rs::parse::TopResultType;

        let mut s = Song::default();
        let result_name = r.result_name.clone();
        s.metadata.insert("title".into(), vec![r.result_name]);

        if let Some(artist) = r.artist {
            s.metadata.insert("artist".into(), vec![artist]);
        }
        if let Some(thumb) = r.thumbnails.last() {
            s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
        }

        match r.result_type {
            Some(TopResultType::Artist) => {
                // P1 fix: Handle Artists without browse_id
                // Some Artists in TopResults don't have browse_id but we can still display them
                if let Some(browse_id) = r.browse_id {
                    s.uri = format!("artist:{}", browse_id);
                } else {
                    // Use sanitized name as fallback ID - user can still see the artist
                    // but browsing won't work
                    log::warn!("TopResult Artist '{}' has no browse_id, using name as fallback", result_name);
                    s.uri = format!("artist:name:{}", result_name.replace(' ', "_"));
                    s.metadata.insert("browsable".into(), vec!["false".into()]);
                }
                s.metadata.insert("type".into(), vec!["artist".into()]);
            }
            Some(TopResultType::Album(_)) => {
                if let Some(browse_id) = r.browse_id {
                    s.uri = format!("album:{}", browse_id);
                } else {
                    log::warn!("TopResult Album '{}' has no browse_id", result_name);
                    return None;
                }
                s.metadata.insert("type".into(), vec!["album".into()]);
            }
            Some(TopResultType::Song) | Some(TopResultType::Video) => {
                if let Some(video_id) = r.video_id {
                    s.uri = video_id;
                } else {
                    log::warn!("TopResult Song/Video '{}' has no video_id", result_name);
                    return None;
                }
                s.metadata.insert("type".into(), vec!["song".into()]);
            }
            Some(TopResultType::Playlist) => {
                if let Some(browse_id) = r.browse_id {
                    s.uri = format!("playlist:{}", browse_id);
                } else {
                    log::warn!("TopResult Playlist '{}' has no browse_id", result_name);
                    return None;
                }
                s.metadata.insert("type".into(), vec!["playlist".into()]);
            }
            _ => {
                // Fallback: try video_id first, then browse_id
                if let Some(id) = r.video_id {
                    s.uri = id;
                    s.metadata.insert("type".into(), vec!["song".into()]);
                } else if let Some(id) = r.browse_id {
                    s.uri = format!("unknown:{}", id);
                    s.metadata.insert("type".into(), vec!["unknown".into()]);
                } else {
                    log::warn!("TopResult '{}' has no video_id or browse_id", result_name);
                    return None;
                }
            }
        }

        Some(s)
    }



    fn parse_video_result(&self, video: SearchResultVideo) -> Option<Song> {
        match video {
            SearchResultVideo::Video { title, channel_name, video_id, length, thumbnails, .. } => {
                let mut s = Song::default();
                s.uri = video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![title]);
                s.metadata.insert("artist".into(), vec![channel_name]);
                s.metadata.insert("type".into(), vec!["video".into()]);
                if let Some(thumb) = thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                s.duration = Self::parse_duration(&length);
                Some(s)
            }
            SearchResultVideo::VideoEpisode {
                title,
                channel_name,
                episode_id,
                thumbnails,
                ..
            } => {
                let mut s = Song::default();
                s.uri = episode_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![title]);
                s.metadata.insert("artist".into(), vec![channel_name]);
                s.metadata.insert("type".into(), vec!["episode".into()]);
                if let Some(thumb) = thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                Some(s)
            }
        }
    }

    fn parse_playlist_result(&self, pl: BasicSearchResultCommunityPlaylist) -> Option<Song> {
        match pl {
            BasicSearchResultCommunityPlaylist::Playlist(p) => {
                let mut s = Song::default();
                s.uri = format!("playlist:{}", p.playlist_id.get_raw());
                s.metadata.insert("title".into(), vec![p.title]);
                s.metadata.insert("artist".into(), vec![p.author]);
                s.metadata.insert("type".into(), vec!["playlist".into()]);
                if let Some(thumb) = p.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                Some(s)
            }
            BasicSearchResultCommunityPlaylist::Podcast(p) => {
                let mut s = Song::default();
                s.uri = format!("podcast:{}", p.podcast_id.get_raw());
                s.metadata.insert("title".into(), vec![p.title]);
                s.metadata.insert("artist".into(), vec![p.publisher]);
                s.metadata.insert("type".into(), vec!["podcast".into()]);
                if let Some(thumb) = p.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                Some(s)
            }
            _ => None,
        }
    }

    /// Parse duration string "MM:SS" or "HH:MM:SS" to Duration
    fn parse_duration(s: &str) -> Option<Duration> {
        s.split(':')
            .try_fold(0u64, |acc, part| part.parse::<u64>().map(|v| acc * 60 + v).ok())
            .map(Duration::from_secs)
    }
}

impl Default for YouTubeApi {
    fn default() -> Self {
        Self::new().expect("Failed to create YouTubeApi")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(YouTubeApi::parse_duration("3:45"), Some(Duration::from_secs(225)));
        assert_eq!(YouTubeApi::parse_duration("1:23:45"), Some(Duration::from_secs(5025)));
        assert_eq!(YouTubeApi::parse_duration("0:30"), Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_not_authenticated_by_default() {
        let api = YouTubeApi::new().unwrap();
        assert!(!api.is_authenticated());
    }

    // NOTE: parse_top_result_tests are disabled because ytmapi_rs::parse::TopResult
    // is marked as #[non_exhaustive] and cannot be constructed in test code.
    // These tests would require the ytmapi_rs crate to expose a builder or
    // test helper for TopResult.
    #[cfg(feature = "ytmapi_test_helpers")]
    mod parse_top_result_tests {
        use super::*;
        use ytmapi_rs::common::Thumbnail;
        use ytmapi_rs::parse::{TopResult, TopResultType};

        fn create_test_api() -> YouTubeApi {
            YouTubeApi::new().unwrap()
        }

        #[test]
        fn test_artist_with_browse_id() {
            let api = create_test_api();
            let result = TopResult {
                result_name: "Test Artist".into(),
                result_type: Some(TopResultType::Artist),
                thumbnails: vec![],
                artist: None,
                album: None,
                duration: None,
                year: None,
                subscribers: Some("1K subscribers".into()),
                plays: None,
                publisher: None,
                byline: None,
                browse_id: Some("UC12345".into()),
                video_id: None,
            };

            let song = api.parse_top_result(result).unwrap();
            assert_eq!(song.uri, "artist:UC12345");
            assert_eq!(song.metadata.get("type"), Some(&vec!["artist".into()]));
        }

        #[test]
        fn test_artist_without_browse_id_uses_fallback() {
            let api = create_test_api();
            let result = TopResult {
                result_name: "KIMLONG".into(),
                result_type: Some(TopResultType::Artist),
                thumbnails: vec![],
                artist: None,
                album: None,
                duration: None,
                year: None,
                subscribers: Some("4.47K subscribers".into()),
                plays: None,
                publisher: None,
                byline: None,
                browse_id: None,  // No browse_id - this is the P1 bug case
                video_id: None,
            };

            let song = api.parse_top_result(result);
            // P1 fix: Should not return None, should use fallback
            assert!(song.is_some(), "Artist without browse_id should still parse");
            
            let song = song.unwrap();
            assert_eq!(song.uri, "artist:name:KIMLONG");
            assert_eq!(song.metadata.get("type"), Some(&vec!["artist".into()]));
            assert_eq!(song.metadata.get("browsable"), Some(&vec!["false".into()]));
        }

        #[test]
        fn test_song_without_video_id_returns_none() {
            let api = create_test_api();
            let result = TopResult {
                result_name: "Test Song".into(),
                result_type: Some(TopResultType::Song),
                thumbnails: vec![],
                artist: Some("Test Artist".into()),
                album: None,
                duration: None,
                year: None,
                subscribers: None,
                plays: None,
                publisher: None,
                byline: None,
                browse_id: None,
                video_id: None,  // Songs need video_id
            };

            let song = api.parse_top_result(result);
            assert!(song.is_none(), "Song without video_id should return None");
        }
    }
}
