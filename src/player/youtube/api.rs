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

    /// Search for music
    pub fn search(&self, query: &str) -> Result<Vec<Song>> {
        let raw_query = query;
        let query = match Self::sanitize_query(raw_query) {
            Some(q) => q,
            None => {
                log::debug!("YouTube API: search called with empty/invalid query, skipping");
                return Ok(Vec::new());
            }
        };

        log::debug!("YouTube API: search(query='{}', raw='{}')", query, raw_query);
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;

        let query_for_log = query.clone();
        let results = self
            .rt
            .block_on(async move {
                let search_query = SearchQuery::new(query);
                api.query(search_query).await
            })
            .map_err(|e| {
                log::error!("YouTube API search failed for query='{}': {}", query_for_log, e);
                e
            })?;

        let mut songs = Vec::new();

        // Helper to add section header
        let add_header = |songs: &mut Vec<Song>, title: &str| {
            let mut s = Song::default();
            s.metadata.insert("type".into(), vec!["header".into()]);
            s.metadata.insert("title".into(), vec![title.into()]);
            songs.push(s);
            log::debug!("Added section header: '{}'", title);
        };

        // Log search results structure
        log::info!("Search results for '{}': top_results={}, artists={}, albums={}, songs={}, videos={}, playlists={}",
            query_for_log,
            results.top_results.len(),
            results.artists.len(),
            results.albums.len(),
            results.songs.len(),
            results.videos.len(),
            results.community_playlists.len()
        );

        // Top results
        if !results.top_results.is_empty() {
            log::debug!("Processing {} top results", results.top_results.len());
            add_header(&mut songs, "Top Result");
            for (idx, r) in results.top_results.iter().enumerate() {
                log::debug!("  Top result {}: name='{}', type={:?}, browse_id={:?}, video_id={:?}",
                    idx, r.result_name, r.result_type, r.browse_id, r.video_id);
                if let Some(song) = self.parse_top_result(r.clone()) {
                    log::debug!("    ✓ Parsed successfully: file={}, type={:?}",
                        song.file, song.metadata.get("type"));
                    songs.push(song);
                } else {
                    log::warn!("    ✗ Failed to parse top result: {:?}", r);
                }
            }
        } else {
            log::warn!("No top results returned from YouTube API");
        }

        // Artists
        if !results.artists.is_empty() {
            add_header(&mut songs, "Artists");
            for a in results.artists {
                let mut s = Song::default();
                s.file = format!("artist:{}", a.browse_id.get_raw());
                s.metadata.insert("title".into(), vec![a.artist]);
                s.metadata.insert("type".into(), vec!["artist".into()]);
                if let Some(subs) = a.subscribers {
                    s.metadata.insert("subtitle".into(), vec![subs]);
                }
                if let Some(thumb) = a.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                songs.push(s);
            }
        }

        // Albums
        if !results.albums.is_empty() {
            add_header(&mut songs, "Albums");
            for a in results.albums {
                let mut s = Song::default();
                s.file = format!("album:{}", a.album_id.get_raw());
                s.metadata.insert("title".into(), vec![a.title]);
                s.metadata.insert("artist".into(), vec![a.artist]);
                s.metadata.insert("year".into(), vec![a.year]);
                s.metadata.insert("type".into(), vec!["album".into()]);
                if let Some(thumb) = a.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                songs.push(s);
            }
        }

        // Songs
        if !results.songs.is_empty() {
            add_header(&mut songs, "Songs");
            for song in results.songs {
                let mut s = Song::default();
                s.file = song.video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![song.title]);
                s.metadata.insert("artist".into(), vec![song.artist]);
                if let Some(album) = song.album {
                    s.metadata.insert("album".into(), vec![album.name]);
                }
                s.metadata.insert("type".into(), vec!["song".into()]);
                if let Some(thumb) = song.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                s.duration = Self::parse_duration(&song.duration);
                songs.push(s);
            }
        }

        // Videos
        if !results.videos.is_empty() {
            add_header(&mut songs, "Videos");
            for video in results.videos {
                if let Some(s) = self.parse_video_result(video) {
                    songs.push(s);
                }
            }
        }

        // Community playlists
        if !results.community_playlists.is_empty() {
            add_header(&mut songs, "Playlists");
            for pl in results.community_playlists {
                if let Some(s) = self.parse_playlist_result(pl) {
                    songs.push(s);
                }
            }
        }

        Ok(songs)
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
            s.file = track.video_id.get_raw().to_string();
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
                s.file = song.video_id.get_raw().to_string();
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
                s.file = format!("album:{}", album.album_id.get_raw());
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
            s.file = track.video_id.get_raw().to_string();
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

    fn parse_top_result(&self, r: ytmapi_rs::parse::TopResult) -> Option<Song> {
        use ytmapi_rs::parse::TopResultType;

        let mut s = Song::default();
        s.metadata.insert("title".into(), vec![r.result_name]);

        if let Some(artist) = r.artist {
            s.metadata.insert("artist".into(), vec![artist]);
        }
        if let Some(thumb) = r.thumbnails.last() {
            s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
        }

        match r.result_type {
            Some(TopResultType::Artist) => {
                s.file = format!("artist:{}", r.browse_id?);
                s.metadata.insert("type".into(), vec!["artist".into()]);
            }
            Some(TopResultType::Album(_)) => {
                s.file = format!("album:{}", r.browse_id?);
                s.metadata.insert("type".into(), vec!["album".into()]);
            }
            Some(TopResultType::Song) | Some(TopResultType::Video) => {
                s.file = r.video_id?;
                s.metadata.insert("type".into(), vec!["song".into()]);
            }
            Some(TopResultType::Playlist) => {
                s.file = format!("playlist:{}", r.browse_id?);
                s.metadata.insert("type".into(), vec!["playlist".into()]);
            }
            _ => {
                // Fallback
                if let Some(id) = r.video_id {
                    s.file = id;
                    s.metadata.insert("type".into(), vec!["song".into()]);
                } else {
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
                s.file = video_id.get_raw().to_string();
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
                s.file = episode_id.get_raw().to_string();
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
                s.file = format!("playlist:{}", p.playlist_id.get_raw());
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
                s.file = format!("podcast:{}", p.podcast_id.get_raw());
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
}
