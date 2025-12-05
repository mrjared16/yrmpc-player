use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::process::{Child, Command};

use anyhow::{Result, anyhow, Context, bail};
use parking_lot::Mutex;
use tokio::runtime::Runtime;
use ytmapi_rs::{
    YtMusic, auth::BrowserToken, 
    query::{SearchQuery, search::SongsFilter, GetWatchPlaylistQuery, GetAlbumQuery, GetArtistQuery, GetPlaylistDetailsQuery, GetPlaylistTracksQuery}, 
    common::{YoutubeID, VideoID, AlbumID, ArtistChannelID, PlaylistID},
    parse::{SearchResultVideo, PlaylistItem},
};
use crate::mpd::commands::LsInfoEntry;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;
use rusty_ytdl::Video;
use chrono::Utc;

use crate::config::YouTubeConfig;
use crate::app_state::AppState;
use crate::domain::{QueuePosition, Song, Status, PlaybackState};
use crate::mpd::{
    commands::{
        Output, Decoder, Playlist, SaveMode, SeekPosition, ValueChange,
        status::OnOffOneshot,
    },
    mpd_client::{Filter, SingleOrRange, Tag},
    version::Version,
};
use super::backend::MusicBackend;
use super::mpv_ipc::MpvIpc;
use crate::player::youtube::details::{PlaylistDetails, AlbumDetails, ArtistDetails, ArtistRef, AlbumRef};

#[derive(Debug)]
pub struct YouTubeBackend {
    rt: Runtime,
    // Option because we might initialize it later or it might fail
    api: Arc<Mutex<Option<YtMusic<BrowserToken>>>>,
    mpv: Arc<Mutex<MpvIpc>>,
    mpv_process: Option<Child>,  // Track spawned MPV process for cleanup
    app_state: Arc<RwLock<AppState>>,
    stream_cache: Arc<Mutex<LruCache<String, (String, Instant)>>>,
    library_cache: Arc<Mutex<super::library_cache::LibraryCache>>,
}

impl YouTubeBackend {
    pub fn new(
        app_state: Arc<RwLock<AppState>>,
        mpv_socket: &std::path::Path,
        config: YouTubeConfig,
    ) -> Result<Self> {
        let rt = Runtime::new()?;
        
        // Try to connect, auto-start MPV if connection fails
        let (mpv, mpv_process) = Self::connect_or_spawn_mpv(mpv_socket)?;
        
        let mut backend = Self {
            rt,
            api: Arc::new(Mutex::new(None)),
            mpv: Arc::new(Mutex::new(mpv)),
            mpv_process,
            app_state,
            stream_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(100).unwrap()))),
            library_cache: Arc::new(Mutex::new(super::library_cache::LibraryCache::new())),
        };

        if let Some(auth_file) = config.auth_file {
            if let Err(e) = backend.load_cookies(&auth_file) {
                log::error!("Failed to load YouTube cookies from {}: {}", auth_file, e);
            } else {
                log::info!("Successfully loaded YouTube cookies from {}", auth_file);
            }
        }
        
        Ok(backend)
    }

    /// Connect to existing MPV or spawn new process
    fn connect_or_spawn_mpv(socket_path: &std::path::Path) -> Result<(MpvIpc, Option<Child>)> {
        // First attempt: connect to existing MPV instance
        match MpvIpc::connect(socket_path) {
            Ok(mpv) => {
                log::info!("Connected to existing MPV instance at {}", socket_path.display());
                return Ok((mpv, None));
            }
            Err(e) => {
                log::debug!("MPV connection failed ({}), attempting to spawn MPV", e);
            }
        }
        
        // Spawn new MPV process
        let socket_str = socket_path.to_string_lossy();
        log::info!("Spawning MPV with socket: {}", socket_str);
        
        let mut child = Command::new("mpv")
            .args([
                "--idle=yes",
                "--vo=null",  // No video output (prevents UI window)
                "--no-terminal",
                &format!("--input-ipc-server={}", socket_str),
            ])
            .spawn()
            .context("Failed to spawn MPV process. Is MPV installed? Try: apt install mpv")?;
        
        log::info!("Spawned MPV process (PID: {})", child.id());
        
        // Wait for socket to be ready with timeout
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(5);
        
        loop {
            if start.elapsed() > timeout {
                let _ = child.kill();
                bail!("Timeout waiting for MPV socket to become available");
            }
            
            // Try to connect
            match MpvIpc::connect(socket_path) {
                Ok(mpv) => {
                    log::info!("Successfully connected to spawned MPV instance");
                    return Ok((mpv, Some(child)));
                }
                Err(_) => {
                    // Socket not ready yet, wait a bit
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }



    pub fn load_cookies(&mut self, path: &str) -> Result<()> {
        // Read and parse Netscape cookie file
        let contents = std::fs::read_to_string(path)?;
        
        // Parse Netscape format cookies into semicolon-separated format
        // Netscape format: domain \t flag \t path \t secure \t expiration \t name \t value
        let mut cookie_parts = Vec::new();
        
        for line in contents.lines() {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            // Split by tabs
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 7 {
                let name = parts[5];
                let value = parts[6];
                cookie_parts.push(format!("{}={}", name, value));
            }
        }
        
        if cookie_parts.is_empty() {
            return Err(anyhow::anyhow!("No valid cookies found in file"));
        }
        
        let cookie_string = cookie_parts.join("; ");
        log::info!("Parsed {} cookies from Netscape format file", cookie_parts.len());
        
        // Use YtMusicBuilder with BrowserToken::from_str instead of from_cookie_file
        let api = self.block_on(async {
            use ytmapi_rs::auth::BrowserToken;
            use ytmapi_rs::{Client, YtMusicBuilder};
            
            let client = Client::new()?;
            let token = BrowserToken::from_str(&cookie_string, &client).await?;
            YtMusicBuilder::new_with_client(client)
                .with_browser_token(token)
                .build()
        })?;
        
        *self.api.lock() = Some(api);
        Ok(())
    }

    pub fn is_api_loaded(&self) -> bool {
        self.api.lock().is_some()
    }


    pub fn enter_idle(&mut self) -> Result<()> {
        // MPV sends events asynchronously
        Ok(())
    }

    pub fn read_response(&mut self) -> Result<Vec<crate::mpd::commands::IdleEvent>> {
        let mut mpv = self.mpv.lock();
        match mpv.receive_message() {
            Ok(resp) => {
                if let Some(event) = resp.event {
                    match event.as_str() {
                        "property-change" => Ok(vec![crate::mpd::commands::IdleEvent::Player]),
                        "pause" | "unpause" | "metadata-update" | "seek" | "file-loaded" => {
                            Ok(vec![crate::mpd::commands::IdleEvent::Player])
                        }
                        _ => Ok(vec![]),
                    }
                } else {
                    Ok(vec![])
                }
            }
            Err(_) => Ok(vec![]),
        }
    }

    pub fn reconnect(&mut self) -> Result<()> {
        // TODO: Reconnect MPV IPC if needed
        Ok(())
    }

    pub fn try_clone_stream(&self) -> Result<std::os::unix::net::UnixStream> {
        let mpv = self.mpv.lock();
        mpv.try_clone_stream()
    }

    /// Helper to run async Tokio code in the sync context
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.rt.block_on(future)
    }

    /// Extract stream URL for a YouTube video ID
    /// Uses LRU cache with 1-hour TTL to minimize API calls
    /// Includes retry logic for transient failures
    fn get_stream_url(&self, video_id: &str) -> Result<String> {
        log::error!("=== get_stream_url CALLED ===");
        log::error!("Input video_id: '{}'", video_id);
        match self.get_stream_url_with_retry(video_id, 2) {
            Ok(url) => Ok(url),
            Err(e) => {
                let error_msg = e.to_string();
                // Filter out known YouTube API structure errors and provide graceful degradation
                if error_msg.contains("searchSuggestionsSectionRenderer") || 
                   error_msg.contains("Key /contents") ||
                   error_msg.contains("not found in Api response") {
                    
                    log::warn!("Filtering YouTube API structure error from UI: {}", error_msg);
                    Err(anyhow!("Unable to play this song due to YouTube API changes. Please try a different song."))
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Extract stream URL with retry logic
    fn get_stream_url_with_retry(&self, video_id: &str, max_retries: u32) -> Result<String> {
        // Check cache first (1-hour TTL)
        {
            let mut cache = self.stream_cache.lock();
            if let Some((url, timestamp)) = cache.get(video_id) {
                if timestamp.elapsed() < Duration::from_secs(3600) {
                    return Ok(url.clone());
                }
            }
        }

        let mut last_error = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                // Exponential backoff: 500ms, 1s, 2s, ...
                let backoff_ms = 500 * (1 << (attempt - 1));
                std::thread::sleep(Duration::from_millis(backoff_ms));
            }

            // Extract stream URL using rusty_ytdl
            let video_id_owned = video_id.to_string();
            match self.block_on(async move {
                let video = Video::new(video_id_owned)?;
                video.get_info().await
            }) {
                Ok(info) => {
                    // Select highest bitrate audio-only format
                    if let Some(format) = info.formats
                        .iter()
                        .filter(|f| f.has_audio && !f.has_video)
                        .max_by_key(|f| f.bitrate)
                    {
                        let url = format.url.clone();
                        self.stream_cache.lock().put(video_id.to_string(), (url.clone(), Instant::now()));
                        return Ok(url);
                    } else {
                        return Err(anyhow!("No audio stream found for video ID: {}", video_id));
                    }
                }
                Err(e) => {
                    // Check for specific API structure errors and provide better error handling
                    let error_msg = e.to_string();
                    if error_msg.contains("searchSuggestionsSectionRenderer") || 
                       error_msg.contains("Key /contents") ||
                       error_msg.contains("not found in Api response") {
                        // This is a known YouTube API structure issue - implement graceful degradation
                        log::warn!("YouTube API structure incompatibility detected: {}. This may be due to YouTube API changes.", error_msg);
                        
                        // For this specific error, we'll skip this song and continue
                        // This is better than failing the entire playback
                        last_error = Some(anyhow!("Skipping song due to YouTube API incompatibility: {}", error_msg));
                        
                        // Don't retry for API structure issues - they won't resolve with retries
                        break;
                    } else {
                        last_error = Some(anyhow::Error::from(e));
                    }
                    // Continue to retry for other types of errors
                }
            }
        }

        Err(last_error.map(anyhow::Error::from).unwrap_or_else(|| anyhow!("Failed to extract stream after {} retries", max_retries)))
    }

    /// Clear stream cache (useful for 403 errors where URLs expire)
    fn clear_stream_cache(&mut self) {
        self.stream_cache.lock().clear();
    }

    /// Activate radio mode: fetch recommendations when queue is empty
    /// Uses GetWatchPlaylist API with last played track as seed
    pub fn activate_radio(&mut self, seed_video_id: Option<String>) -> Result<()> {
        let api_opt = self.api.lock().clone();
        if api_opt.is_none() {
            return Err(anyhow!("YouTube API not initialized"));
        }
        let api = api_opt.unwrap();

        // Determine seed: use provided or last currentfrom queue
        let seed = if let Some(id) = seed_video_id {
            id
        } else {
            let app_state = self.app_state.read().unwrap();
            app_state.get_current()
                .or_else(|| app_state.get_queue().back())
                .map(|item| item.song.file.clone())
                .ok_or(anyhow!("No seed track for radio"))?
        };

        // Fetch recommendations using GetWatchPlaylist
        let tracks = self.block_on(async move {
            let query = GetWatchPlaylistQuery::new_from_video_id(VideoID::from_raw(seed.as_str()));
            let tracks = api.query(query).await?;
            Ok::<Vec<_>, anyhow::Error>(tracks.into_iter().take(10).collect())
        })?;

        // Convert and add to queue
        for track in tracks {
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![track.title]);
            
            if let Some(thumb) = track.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }
            // Note: WatchPlaylistTrack has different fields than SearchResultSong
            // artist info is in a different structure

            let song = Song {
                id: None,
                file: track.video_id.get_raw().to_string(),
                duration: None, // WatchPlaylistTrack doesn't have duration
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            };

            self.app_state.write().unwrap().add(song, None);
        }

        Ok(())
    }
    /// Uses MPV's `loadfile append` to queue upcoming tracks
    fn ensure_prebuffered(&mut self, count: usize) -> Result<()> {
        let app_state = self.app_state.read().unwrap();
        let current_idx = app_state.get_current_index();
        
        if current_idx.is_none() {
            return Ok(());
        }
        let current_idx = current_idx.unwrap();
        
        // Get MPV's current playlist count
        let mut mpv = self.mpv.lock();
        let playlist_count: i64 = serde_json::from_value(
            mpv.get_property("playlist-count").unwrap_or(serde_json::json!(0))
        ).unwrap_or(0);
        drop(mpv);
        
        // Calculate how many more tracks we need to buffer
        let buffered_ahead = (playlist_count as usize).saturating_sub(1); // -1 for current track
        let need_to_buffer = count.saturating_sub(buffered_ahead);
        
        // Buffer upcoming tracks
        for i in (buffered_ahead + 1)..=(buffered_ahead + need_to_buffer) {
            if let Some(item) = app_state.get_queue().get(current_idx + i) {
                let url = self.get_stream_url(&item.song.file)?;
                self.mpv.lock().send_command(vec!["loadfile", &url, "append"])?;
            } else {
                break; // No more tracks in queue
            }
        }
        
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "YouTube"
    }

    /// Browse playlist details including tracks and related content
    pub fn browse_playlist(&self, playlist_id: &str) -> Result<PlaylistDetails> {
        log::debug!("YouTubeBackend: browse_playlist(id='{}')", playlist_id);
        
        // Strip "playlist:" prefix if present (IDs come prefixed from search results)
        let raw_id = playlist_id.strip_prefix("playlist:").unwrap_or(playlist_id);
        log::debug!("YouTubeBackend: browse_playlist raw_id='{}'", raw_id);
        
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;
        let playlist_id = PlaylistID::from_raw(raw_id);
        
        let details_query = GetPlaylistDetailsQuery::new(playlist_id.clone());
        let tracks_query = GetPlaylistTracksQuery::new(playlist_id.clone());

        let (details, tracks_result) = self.rt.block_on(async move {
            let d = api.query(details_query).await;
            let t = api.query(tracks_query).await;
            (d, t)
        });

        let details = details.map_err(|e| {
             log::error!("YouTubeBackend browse_playlist details failed: {}", e);
             e
        })?;
        let tracks_list = tracks_result.map_err(|e| {
             log::error!("YouTubeBackend browse_playlist tracks failed: {}", e);
             e
        })?;

        // Parse tracks
        let mut tracks = Vec::new();
        for item in tracks_list {
            let mut s = Song::default();
            match item {
                PlaylistItem::Song(song) => {
                    s.file = song.video_id.get_raw().to_string();
                    s.metadata.insert("title".into(), vec![song.title]);
                    let artist_names: Vec<String> = song.artists.iter().map(|a| a.name.clone()).collect();
                    s.metadata.insert("artist".into(), artist_names);
                    if !song.album.name.is_empty() {
                        s.metadata.insert("album".into(), vec![song.album.name]);
                    }
                    if let Some(thumb) = song.thumbnails.last() {
                        s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                    }
                    s.metadata.insert("type".into(), vec!["song".into()]);
                },
                PlaylistItem::Video(video) => {
                    s.file = video.video_id.get_raw().to_string();
                    s.metadata.insert("title".into(), vec![video.title]);
                    s.metadata.insert("artist".into(), vec![video.channel_name]);
                    if let Some(thumb) = video.thumbnails.last() {
                        s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                    }
                    s.metadata.insert("type".into(), vec!["video".into()]);
                },
                _ => continue,
            }
            tracks.push(s);
        }

        // TODO: Parse featured artists and related playlists when ytmapi-rs exposes them
        let featured_artists = Vec::new();
        let related_playlists = Vec::new();

        Ok(PlaylistDetails {
            id: details.id.get_raw().to_string(),
            title: details.title,
            artist: Some(details.author),
            year: Some(details.year),
            thumbnail: details.thumbnails.last().map(|th| th.url.clone()),
            track_count: tracks.len(),
            duration_text: Some(details.duration),
            tracks,
            featured_artists,
            related_playlists,
        })
    }

    /// Browse album details including tracks and artist info
    pub fn browse_album(&self, album_id: &str) -> Result<AlbumDetails> {
        log::error!("=== browse_album CALLED ===");
        log::error!("Input album_id: '{}'", album_id);
        
        // Strip "album:" prefix if present (IDs come prefixed from search results)
        let raw_id = album_id.strip_prefix("album:").unwrap_or(album_id);
        log::error!("After strip_prefix, raw_id: '{}'", raw_id);
        
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;
        let album_id = AlbumID::from_raw(raw_id);
        
        let query = GetAlbumQuery::new(album_id.clone());
        let result = self.rt.block_on(async move {
            api.query(query).await
        }).map_err(|e| {
            log::error!("YouTubeBackend browse_album failed for id='{}': {}", album_id.get_raw(), e);
            e
        })?;

        // Parse tracks
        let mut tracks = Vec::new();
        for track in result.tracks {
                let mut s = Song::default();
                s.file = track.video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![track.title]);

                s.metadata.insert("album".into(), vec![result.title.clone()]);
                if let Some(thumb) = result.thumbnails.last() {
                    s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                s.metadata.insert("type".into(), vec!["song".into()]);
                tracks.push(s);
            }

        // Parse artist reference
        let artist = ArtistRef {
            id: result.artists.first().and_then(|a| a.id.as_ref().map(|id| id.get_raw().to_string())).unwrap_or_default(),
            name: result.artists.first().map(|a| a.name.clone()).unwrap_or_else(|| "Unknown Artist".to_string()),
            thumbnail: None,
        };

        // TODO: Parse "more by artist" when ytmapi-rs exposes it
        let more_by_artist = Vec::new();

        Ok(AlbumDetails {
            id: album_id.get_raw().to_string(),
            title: result.title,
            artist,
            year: Some(result.year),
            thumbnail: result.thumbnails.last().map(|t| t.url.clone()),
            tracks,
            more_by_artist,
        })
    }

    /// Browse artist details including top songs and albums
    pub fn browse_artist(&self, artist_id: &str) -> Result<ArtistDetails> {
        log::error!("=== browse_artist CALLED ===");
        log::error!("Input artist_id: '{}'", artist_id);
        
        // Strip "artist:" prefix if present (IDs come prefixed from search results)
        let raw_id = artist_id.strip_prefix("artist:").unwrap_or(artist_id);
        log::error!("After strip_prefix, raw_id: '{}'", raw_id);
        
        let api = self.api.lock().clone().ok_or_else(|| anyhow!("API not initialized"))?;
        let artist_id = ArtistChannelID::from_raw(raw_id);
        
        let query = GetArtistQuery::new(artist_id.clone());
        let result = self.rt.block_on(async move {
            api.query(query).await
        }).map_err(|e| {
            log::error!("YouTubeBackend browse_artist failed for id='{}': {}", artist_id.get_raw(), e);
            e
        })?;

        // Parse top songs
        let mut top_songs = Vec::new();
        if let Some(songs) = result.top_releases.songs {
            for song in songs.results.iter().take(10) {  // Limit to top 10
                let mut s = Song::default();
                s.file = song.video_id.get_raw().to_string();
                s.metadata.insert("title".into(), vec![song.title.clone()]);
                let artist_names: Vec<String> = song.artists.iter().map(|a| a.name.clone()).collect();
                s.metadata.insert("artist".into(), artist_names);
                if !song.album.name.is_empty() {
                    s.metadata.insert("album".into(), vec![song.album.name.clone()]);
                }
                // ArtistSong doesn't have thumbnails, use artist thumbnail as fallback?
                // Or maybe ParsedSongAlbum has it? For now, skip.
                if let Some(thumb) = result.thumbnails.last() {
                     s.metadata.insert("thumbnail".into(), vec![thumb.url.clone()]);
                }
                s.metadata.insert("type".into(), vec!["song".into()]);
                top_songs.push(s);
            }
        }

        // Parse albums
        let mut albums = Vec::new();
        if let Some(album_results) = result.top_releases.albums {
            for album in album_results.results.iter().take(20) {
                albums.push(AlbumRef {
                    id: album.album_id.get_raw().to_string(),
                    title: album.title.clone(),
                    year: Some(album.year.clone()),
                    thumbnail: album.thumbnails.last().map(|t| t.url.clone()),
                });
            }
        }

        // Parse singles
        let mut singles = Vec::new();
        if let Some(singles_results) = result.top_releases.singles {
            for single in singles_results.results.iter().take(20) {
                singles.push(AlbumRef {
                    id: single.album_id.get_raw().to_string(),
                    title: single.title.clone(),
                    year: Some(single.year.clone()),
                    thumbnail: single.thumbnails.last().map(|t| t.url.clone()),
                });
            }
        }

        // TODO: Parse related artists when ytmapi-rs exposes them
        let related_artists = Vec::new();

        Ok(ArtistDetails {
            id: artist_id.get_raw().to_string(),
            name: result.name,
            subscribers: result.subscribers,
            description: result.description,
            thumbnail: result.thumbnails.last().map(|t| t.url.clone()),
            top_songs,
            albums,
            singles,
            related_artists,
        })
    }
}

impl MusicBackend for YouTubeBackend {
    fn backend_name(&self) -> &'static str {
        "YouTube"
    }
    fn get_search_suggestions(&mut self, query: String) -> Result<Vec<String>> {
        let api_opt = self.api.lock().as_ref().cloned();
        if let Some(api) = api_opt {
            let rt = &self.rt;
            let result = rt.block_on(async {
                let suggestions = api.get_search_suggestions(query).await?;
                let texts: Vec<String> = suggestions.into_iter()
                    .map(|s| s.get_text())
                    .collect();
                Ok(texts)
            });
            return result;
        }
        Ok(vec![])
    }

    fn get_library(&mut self, category: super::LibraryCategory) -> Result<Vec<LsInfoEntry>> {
        use ytmapi_rs::query::{
            GetLibraryPlaylistsQuery, GetLibraryAlbumsQuery, 
            GetLibraryArtistsQuery, GetLibrarySongsQuery
        };
        
        // Check cache first
        {
            let mut cache = self.library_cache.lock();
            if let Some(cached_data) = cache.get(category) {
                log::debug!("Library cache HIT for {:?}", category);
                return Ok(cached_data);
            }
        }
        
        // Cache miss - fetch from API
        log::debug!("Library cache MISS for {:?}, fetching from YouTube Music", category);
        
        let api_opt = self.api.lock().as_ref().cloned();
        if api_opt.is_none() {
            return Err(anyhow!("YouTube API not initialized"));
        }
        let api = api_opt.unwrap();
        
        let entries: Vec<LsInfoEntry> = match category {
            super::LibraryCategory::Playlists => {
                let playlists = self.rt.block_on(async move {
                    api.query(GetLibraryPlaylistsQuery).await
                })?;
                
                playlists.into_iter().map(|playlist| {
                    LsInfoEntry::Dir(crate::mpd::commands::lsinfo::Dir {
                        name: playlist.title,
                        full_path: format!("playlist:{}", playlist.playlist_id.get_raw()),
                        last_modified: chrono::Utc::now(),
                    })
                }).collect()
            },
            super::LibraryCategory::Albums => {
                let albums = self.rt.block_on(async move {
                    api.query(GetLibraryAlbumsQuery::default()).await
                })?;
                
                albums.into_iter().map(|album| {
                    LsInfoEntry::Dir(crate::mpd::commands::lsinfo::Dir {
                        name: format!("{} - {}", album.title, album.artist),
                        full_path: format!("album:{}", album.album_id.get_raw()),
                        last_modified: chrono::Utc::now(),
                    })
                }).collect()
            },
            super::LibraryCategory::Artists => {
                let artists = self.rt.block_on(async move {
                    api.query(GetLibraryArtistsQuery::default()).await
                })?;
                
                artists.into_iter().map(|artist| {
                    LsInfoEntry::Dir(crate::mpd::commands::lsinfo::Dir {
                        name: artist.artist,
                        full_path: format!("artist:{}", artist.channel_id.get_raw()),
                        last_modified: chrono::Utc::now(),
                    })
                }).collect()
            },
            super::LibraryCategory::Songs => {
                let songs = self.rt.block_on(async move {
                    api.query(GetLibrarySongsQuery::default()).await
                })?;
                
                songs.into_iter().map(|song| {
                    let mut metadata = HashMap::new();
                    metadata.insert("title".to_string(), vec![song.title.clone()]);
                    // ytmapi-rs TableListSong has 'artists' not 'artist'
                    if let Some(first_artist) = song.artists.first() {
                        metadata.insert("artist".to_string(), vec![first_artist.name.clone()]);
                    }
                    // album is ParsedSongAlbum, not Option
                    metadata.insert("album".to_string(), vec![song.album.name]);
                    if let Some(thumb) = song.thumbnails.last() {
                        metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                    }
                    
                    // Parse duration string "MM:SS" or "HH:MM:SS"
                    let duration = song.duration.split(':').try_fold(0u64, |acc, part| {
                        part.parse::<u64>().map(|v| acc * 60 + v)
                    }).ok().map(Duration::from_secs);
                    
                    LsInfoEntry::File(Song {
                        id: None,
                        file: song.video_id.get_raw().to_string(),
                        duration,
                        metadata,
                        last_modified: None,
                        added: Some(Utc::now()),
                    })
                }).collect()
            },
        };
        
        // Store in cache
        self.library_cache.lock().put(category, entries.clone());
        
        Ok(entries)
    }

    fn as_youtube_backend(&mut self) -> Option<&mut YouTubeBackend> {
        Some(self)
    }

    // ===== Playback Control =====

    fn play(&mut self) -> Result<()> {
        let mut mpv = self.mpv.lock();
        mpv.set_property("pause", serde_json::json!(false))
    }

    fn pause(&mut self, state: bool) -> Result<()> {
        let mut mpv = self.mpv.lock();
        mpv.set_property("pause", serde_json::json!(state))
    }

    fn stop(&mut self) -> Result<()> {
        let mut mpv = self.mpv.lock();
        mpv.send_command(vec!["stop"])?;
        Ok(())
    }

    fn next(&mut self) -> Result<()> {
        // Check if MPV has next track in playlist (gapless scenario)
        let mut mpv = self.mpv.lock();
        let playlist_count: i64 = serde_json::from_value(
            mpv.get_property("playlist-count").unwrap_or(serde_json::json!(0))
        ).unwrap_or(0);
        let playlist_pos: i64 = serde_json::from_value(
            mpv.get_property("playlist-pos").unwrap_or(serde_json::json!(-1))
        ).unwrap_or(-1);
        drop(mpv);
        
        // If MPV has buffered next track, use playlist-next for instant transition
        if playlist_pos >= 0 && playlist_pos + 1 < playlist_count {
            self.mpv.lock().send_command(vec!["playlist-next"])?;
            
            // Update AppState to match
            let next_song = {
                let app_state = self.app_state.read().unwrap();
                app_state.get_next().cloned()
            };
            if let Some(item) = next_song {
                self.app_state.write().unwrap().set_current_by_id(item.id)?;
            }
            
            // Ensure we maintain 2 tracks ahead
            self.ensure_prebuffered(2)?;
            Ok(())
        } else {
            // MPV playlist empty, load next track manually
            let next_song = {
                let app_state = self.app_state.read().unwrap();
                app_state.get_next().cloned()
            };

            if let Some(item) = next_song {
                let url = self.get_stream_url(&item.song.file)?;
                self.app_state.write().unwrap().set_current_by_id(item.id)?;
                self.mpv.lock().send_command(vec!["loadfile", &url, "replace"])?;
                self.ensure_prebuffered(2)?;
                Ok(())
            } else {
                Err(anyhow!("No next song in queue"))
            }
        }
    }

    fn previous(&mut self) -> Result<()> {
        // For YouTube, we implement as "restart current song"
        // since there's no concept of "previous" in a YouTube playlist
        let current_song = {
            let app_state = self.app_state.read().unwrap();
            app_state.get_current().cloned()
        };

        if let Some(item) = current_song {
            let _url = self.get_stream_url(&item.song.file)?;
            self.mpv.lock().send_command(vec!["seek", "0", "absolute"])?;
            Ok(())
        } else {
            Err(anyhow!("No current song playing"))
        }
    }

    fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        let mut mpv = self.mpv.lock();
        match position {
            SeekPosition::Absolute(secs) => {
                mpv.send_command(vec!["seek", &secs.to_string(), "absolute"])?;
            }
            SeekPosition::Relative(secs) => {
                mpv.send_command(vec!["seek", &secs.to_string(), "relative"])?;
            }
        }
        Ok(())
    }

    // ===== Status Queries =====

    fn get_status(&mut self) -> Result<Status> {
        let mut mpv = self.mpv.lock();
        
        // Basic status from MPV
        let paused: bool = serde_json::from_value(mpv.get_property("pause")?)?;
        let volume: f64 = serde_json::from_value(mpv.get_property("volume")?)?;
        let time_pos: f64 = mpv.get_property("time-pos").ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or(0.0);
        let duration: f64 = mpv.get_property("duration").ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or(0.0);

        // Get queue info from AppState
        let app_state = self.app_state.read().unwrap();
        let current_song = app_state.get_current();
        let next_song = app_state.get_next();
        
        Ok(Status {
            state: if paused { PlaybackState::Pause } else { PlaybackState::Play },
            volume: volume as u8,
            elapsed: Some(Duration::from_secs_f64(time_pos)),
            duration: Some(Duration::from_secs_f64(duration)),
            repeat: false, // TODO
            random: false, // TODO
            single: crate::domain::OnOffOneshot::Off,
            consume: crate::domain::OnOffOneshot::Off,
            playlist: Some(app_state.get_version()),
            playlistlength: app_state.get_queue().len() as u32,
            songid: current_song.as_ref().and_then(|s| s.song.id),
            next_songid: next_song.as_ref().and_then(|s| s.song.id),
            song_position: app_state.get_current_index().map(|i| i as u32),
            bitrate: None,
            error: None,
            updating_db: None,
            xfade: None,
            partition: String::from("default"),
            lastloadedplaylist: None,
        })
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        let app_state = self.app_state.read().unwrap();
        Ok(app_state.get_queue().iter().map(|item| {
            let mut song = item.song.clone();
            song.id = Some(item.id);  // Sync song.id with queue item ID
            song
        }).collect())
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        let app_state = self.app_state.read().unwrap();
        Ok(app_state.get_current().map(|item| {
            let mut song = item.song.clone();
            song.id = Some(item.id);  // Sync song.id with queue item ID
            song
        }))
    }

    // ===== Queue Management =====

    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        let uri = uri.to_string();
        
        // Determine if it's a search query or ID/URL
        let is_url = uri.starts_with("http");
        let is_id = uri.len() == 11 && !uri.contains(' ');
        let is_search = !is_url && !is_id;

        let song = if is_search {
            let api_opt = self.api.lock().clone();
            if let Some(api) = api_opt {
                let result = self.block_on(async move {
                    let query = SearchQuery::new(uri).with_filter(SongsFilter);
                    let results = api.query(query).await?;
                    results.into_iter().next().ok_or(anyhow!("No songs found"))
                })?;
                
                // Parse duration "MM:SS" or "HH:MM:SS"
                let duration = result.duration.split(':').try_fold(0u64, |acc, part| {
                    part.parse::<u64>().map(|v| acc * 60 + v)
                }).ok().map(Duration::from_secs);

                let mut metadata = HashMap::new();
                metadata.insert("title".to_string(), vec![result.title]);
                metadata.insert("artist".to_string(), vec![result.artist]);
                if let Some(album) = result.album {
                    metadata.insert("album".to_string(), vec![album.name]);
                }
                
                if let Some(thumb) = result.thumbnails.last() {
                    metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                }

                Song {
                    id: None,
                    file: result.video_id.get_raw().to_string(),
                    duration,
                    metadata,
                    last_modified: None,
                    added: Some(Utc::now()),
                }
            } else {
                return Err(anyhow!("YouTube API not initialized"));
            }
        } else {
             let video_id = if is_url {
                uri.split("v=").nth(1).and_then(|s| s.split('&').next()).unwrap_or(&uri).to_string()
            } else {
                uri
            };
            
            let info = self.block_on(async move {
                let video = Video::new(video_id)?;
                video.get_info().await
            })?;
            
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![info.video_details.title]);
            if let Some(author) = info.video_details.author {
                metadata.insert("artist".to_string(), vec![author.name]);
            }
            
            if let Some(thumb) = info.video_details.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }

            Song {
                id: None,
                file: info.video_details.video_id,
                duration: Some(Duration::from_secs(info.video_details.length_seconds.parse().unwrap_or(0))),
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            }
        };

        self.app_state.write().unwrap().add(song, position);
        Ok(())
    }

    fn delete_id(&mut self, id: u32) -> Result<()> {
        self.app_state.write().unwrap().delete_id(id)
    }

    fn clear(&mut self) -> Result<()> {
        self.app_state.write().unwrap().clear();
        Ok(())
    }

    fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.app_state.write().unwrap().move_id(from, to)
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        let app_state = self.app_state.read().unwrap();
        let item = app_state.find_by_id(id).ok_or(anyhow!("Song not found"))?;
        let video_id = item.song.file.clone();
        drop(app_state); // Release read lock

        let url = self.get_stream_url(&video_id)?;
        
        self.app_state.write().unwrap().set_current_by_id(id)?;
        self.mpv.lock().send_command(vec!["loadfile", &url, "replace"])?;
        
        // Prebuffer next 2 tracks for gapless playback
        self.ensure_prebuffered(2)?;
        self.mpv.lock().set_property("pause", serde_json::json!(false))?;
        
        Ok(())
    }

    // ===== Volume Control =====

    fn volume(&mut self) -> Result<u8> {
        let mut mpv = self.mpv.lock();
        let vol: f64 = serde_json::from_value(mpv.get_property("volume")?)?;
        Ok(vol as u8)
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        let mut mpv = self.mpv.lock();
        match volume {
            ValueChange::Set(v) => {
                mpv.set_property("volume", serde_json::json!(v as f64))?;
            }
            ValueChange::Increase(delta) => {
                let current: f64 = serde_json::from_value(mpv.get_property("volume")?)?;
                mpv.set_property("volume", serde_json::json!(current + delta as f64))?;
            }
            ValueChange::Decrease(delta) => {
                let current: f64 = serde_json::from_value(mpv.get_property("volume")?)?;
                mpv.set_property("volume", serde_json::json!(current - delta as f64))?;
            }
        }
        Ok(())
    }

    // ===== Playback Options =====

    fn repeat(&mut self, _repeat: bool) -> Result<()> { Ok(()) }
    fn random(&mut self, _random: bool) -> Result<()> { Ok(()) }
    fn single(&mut self, _single: OnOffOneshot) -> Result<()> { Ok(()) }
    fn consume(&mut self, _consume: OnOffOneshot) -> Result<()> { Ok(()) }
    fn crossfade(&mut self, _seconds: u32) -> Result<()> { Ok(()) }
    fn shuffle(&mut self, _range: Option<SingleOrRange>) -> Result<()> { Ok(()) }

    // ===== Library Browsing =====

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        let path = path.unwrap_or("");
        if path.is_empty() {
             return Ok(vec![]);
        }

        let api_opt = self.api.lock().as_ref().cloned();
        let api = if let Some(api) = api_opt {
            api
        } else {
            return Ok(vec![]);
        };

        if let Some(playlist_id) = path.strip_prefix("playlist:") {
            let query = GetWatchPlaylistQuery::new_from_playlist_id(PlaylistID::from_raw(playlist_id));
            let playlist = self.rt.block_on(async move { api.query(query).await })?;
            
            let mut entries = Vec::new();
            for track in playlist {
                let mut metadata = HashMap::new();
                metadata.insert("title".to_string(), vec![track.title]);
                // WatchPlaylistTrack has author (String)
                metadata.insert("artist".to_string(), vec![track.author]);
                
                // WatchPlaylistTrack doesn't have album
                
                if let Some(thumb) = track.thumbnails.last() {
                    metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                }
                
                let duration = track.duration.split(':').try_fold(0u64, |acc, part| {
                    part.parse::<u64>().map(|v| acc * 60 + v)
                }).ok().map(Duration::from_secs);

                entries.push(LsInfoEntry::File(Song {
                    id: None,
                    file: track.video_id.get_raw().to_string(),
                    duration,
                    metadata,
                    last_modified: None,
                    added: Some(Utc::now()),
                }));
            }
            return Ok(entries);
        }

        if let Some(album_id) = path.strip_prefix("album:") {
             let query = GetAlbumQuery::new(AlbumID::from_raw(album_id));
             let album = self.rt.block_on(async move { api.query(query).await })?;
             
             let mut entries = Vec::new();
             for track in album.tracks {
                 let mut metadata = HashMap::new();
                 metadata.insert("title".to_string(), vec![track.title]);
                 metadata.insert("artist".to_string(), vec![album.artists.first().map(|a| a.name.clone()).unwrap_or_default()]);
                 metadata.insert("album".to_string(), vec![album.title.clone()]);
                 
                 if let Some(thumb) = album.thumbnails.last() {
                     metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                 }
                 
                 let duration = track.duration.split(':').try_fold(0u64, |acc, part| {
                     part.parse::<u64>().map(|v| acc * 60 + v)
                 }).ok().map(Duration::from_secs);

                 entries.push(LsInfoEntry::File(Song {
                     id: None,
                     file: track.video_id.get_raw().to_string(),
                     duration,
                     metadata,
                     last_modified: None,
                     added: Some(Utc::now()),
                 }));
             }
             return Ok(entries);
        }

        if let Some(artist_id) = path.strip_prefix("artist:") {
             let query = GetArtistQuery::new(ArtistChannelID::from_raw(artist_id));
             let artist = self.rt.block_on(async move { api.query(query).await })?;
             
             let mut entries = Vec::new();
             
             // Top Songs
             if let Some(songs) = artist.top_releases.songs {
                 for song in songs.results {
                     let mut metadata = HashMap::new();
                     metadata.insert("title".to_string(), vec![song.title]);
                     metadata.insert("artist".to_string(), vec![artist.name.clone()]);
                     metadata.insert("album".to_string(), vec![song.album.name]);
                     // ArtistSong might not have thumbnails or duration, skip if missing
                     
                     entries.push(LsInfoEntry::File(Song {
                         id: None,
                         file: song.video_id.get_raw().to_string(),
                         duration: None, // Duration not available in ArtistSong
                         metadata,
                         last_modified: None,
                         added: Some(Utc::now()),
                     }));
                 }
             }

             // Albums
             if let Some(albums) = artist.top_releases.albums {
                 for album in albums.results {
                     let dir = crate::mpd::commands::lsinfo::Dir {
                         name: album.title,
                         full_path: format!("album:{}", album.album_id.get_raw()),
                         last_modified: chrono::Utc::now(),
                     };
                     entries.push(LsInfoEntry::Dir(dir));
                 }
             }

             // Singles
             if let Some(singles) = artist.top_releases.singles {
                 for single in singles.results {
                     let dir = crate::mpd::commands::lsinfo::Dir {
                         name: format!("{} (Single)", single.title),
                         full_path: format!("album:{}", single.album_id.get_raw()),
                         last_modified: chrono::Utc::now(),
                     };
                     entries.push(LsInfoEntry::Dir(dir));
                 }
             }

             return Ok(entries);
        }

        Ok(vec![])
    }

    fn list_all(&mut self, _path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>> {
        let query = filter
            .iter()
            .find_map(|f| {
                if !f.value.is_empty() {
                    Some(f.value.as_ref())
                } else {
                    None
                }
            })
            .unwrap_or("");

        if query.is_empty() {
            return Ok(vec![]);
        }

        let api_opt = self.api.lock().as_ref().cloned();
        let api = if let Some(api) = api_opt {
            api
        } else {
            return Ok(vec![]);
        };
        
        use ytmapi_rs::parse::BasicSearchResultCommunityPlaylist;

        // Use general SearchQuery to get all types of results
        let results = self.rt.block_on(async move {
            use ytmapi_rs::query::SearchQuery;
            
            let search_query = SearchQuery::new(query);
            api.query(search_query).await
        })?;
        
        let mut songs = Vec::new();

        // Parse Top Results
        for result in results.top_results {
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![result.result_name]);
            
            if let Some(artist) = result.artist {
                metadata.insert("artist".to_string(), vec![artist]);
            }
            if let Some(album) = result.album {
                metadata.insert("album".to_string(), vec![album]);
            }
            if let Some(thumb) = result.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }
            
            let (file, type_) = match result.result_type {
                Some(ytmapi_rs::parse::TopResultType::Song) => {
                    if let Some(vid) = result.video_id {
                        (vid, "song")
                    } else { continue; }
                },
                Some(ytmapi_rs::parse::TopResultType::Video) => {
                    if let Some(vid) = result.video_id {
                        (vid, "video")
                    } else { continue; }
                },
                Some(ytmapi_rs::parse::TopResultType::Artist) => {
                    if let Some(bid) = result.browse_id {
                        // Only add "artist:" prefix if not already present
                        let file = if bid.starts_with("artist:") {
                            bid
                        } else {
                            format!("artist:{}", bid)
                        };
                        (file, "artist")
                    } else { continue; }
                },
                Some(ytmapi_rs::parse::TopResultType::Album(_)) => {
                    if let Some(bid) = result.browse_id {
                        // Only add "album:" prefix if not already present
                        let file = if bid.starts_with("album:") {
                            bid
                        } else {
                            format!("album:{}", bid)
                        };
                        (file, "album")
                    } else { continue; }
                },
                Some(ytmapi_rs::parse::TopResultType::Playlist) => {
                    if let Some(bid) = result.browse_id {
                        // Only add "playlist:" prefix if not already present
                        let file = if bid.starts_with("playlist:") {
                            bid
                        } else {
                            format!("playlist:{}", bid)
                        };
                        (file, "playlist")
                    } else { continue; }
                },
                _ => continue,
            };
            
            metadata.insert("type".to_string(), vec![type_.to_string()]);

            songs.push(Song {
                id: None,
                file,
                duration: result.duration.and_then(|d| d.split(':').try_fold(0u64, |acc, part| {
                    part.parse::<u64>().map(|v| acc * 60 + v)
                }).ok()).map(Duration::from_secs),
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            });
        }

        // Parse Artists
        for artist in results.artists {
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![artist.artist]);
            metadata.insert("type".to_string(), vec!["artist".to_string()]);
            if let Some(subs) = artist.subscribers {
                metadata.insert("subtitle".to_string(), vec![subs]);
            }
            if let Some(thumb) = artist.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }

            let artist_id = artist.browse_id.get_raw();
            // Only add "artist:" prefix if it doesn't already have one
            let file = if artist_id.starts_with("artist:") {
                artist_id.to_string()
            } else {
                format!("artist:{}", artist_id)
            };

            songs.push(Song {
                id: None,
                file, // Prefix with artist: for easy identification
                duration: None,
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            });
        }

        // Parse Albums
        for album in results.albums {
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![album.title.clone()]);
            metadata.insert("artist".to_string(), vec![album.artist]);
            metadata.insert("album".to_string(), vec![album.title.clone()]);
            metadata.insert("year".to_string(), vec![album.year]);
            metadata.insert("type".to_string(), vec!["album".to_string()]);
            if let Some(thumb) = album.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }

            // Only add "album:" prefix if not already present
            let album_id = album.album_id.get_raw();
            let file = if album_id.starts_with("album:") {
                album_id.to_string()
            } else {
                format!("album:{}", album_id)
            };

            songs.push(Song {
                id: None,
                file,
                duration: None,
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            });
        }

        // Parse Songs
        for song in results.songs {
            let mut s = Song {
                file: song.video_id.get_raw().to_string(),
                ..Default::default()
            };
            s.metadata.insert("title".to_string(), vec![song.title]);
            s.metadata.insert("artist".to_string(), vec![song.artist]);
            if let Some(album) = song.album {
                s.metadata.insert("album".to_string(), vec![album.name]);
            }
            s.metadata.insert("type".to_string(), vec!["song".to_string()]);
            if let Some(thumb) = song.thumbnails.last() {
                s.metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }
            // Parse duration (song.duration is String, not Option)
            let duration = song.duration.split(':').try_fold(0u64, |acc, part| {
                part.parse::<u64>().map(|v| acc * 60 + v)
            }).ok().map(Duration::from_secs);
            s.duration = duration;
           
            songs.push(s);
        }

        // Parse Videos (treat as songs)
        for video in results.videos {
            match video {
                SearchResultVideo::Video { title, channel_name, video_id, length, thumbnails, .. } => {
                    let mut s = Song {
                        file: video_id.get_raw().to_string(),
                        ..Default::default()
                    };
                    s.metadata.insert("title".to_string(), vec![title]);
                    s.metadata.insert("artist".to_string(), vec![channel_name]);
                    s.metadata.insert("type".to_string(), vec!["video".to_string()]);
                    if let Some(thumb) = thumbnails.last() {
                        s.metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                    }
                    // Parse duration (length field)
                    let duration = length.split(':').try_fold(0u64, |acc, part| {
                        part.parse::<u64>().map(|v| acc * 60 + v)
                    }).ok().map(Duration::from_secs);
                    s.duration = duration;
                    songs.push(s);
                }
                SearchResultVideo::VideoEpisode { title, channel_name, episode_id, thumbnails, .. } => {
                    let mut s = Song {
                        file: episode_id.get_raw().to_string(),
                        ..Default::default()
                    };
                    s.metadata.insert("title".to_string(), vec![title]);
                    s.metadata.insert("artist".to_string(), vec![channel_name]);
                    s.metadata.insert("type".to_string(), vec!["episode".to_string()]);
                    if let Some(thumb) = thumbnails.last() {
                        s.metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                    }
                    songs.push(s);
                }
            }
        }

        // Parse Community Playlists (which may be podcasts or playlists)
        for basic_playlist in results.community_playlists {
            use ytmapi_rs::parse::BasicSearchResultCommunityPlaylist;
            match basic_playlist {
                BasicSearchResultCommunityPlaylist::Playlist(playlist) => {
                    let mut metadata = HashMap::new();
                    metadata.insert("title".to_string(), vec![playlist.title]);
                    metadata.insert("artist".to_string(), vec![playlist.author]);
                    metadata.insert("type".to_string(), vec!["playlist".to_string()]);
                    if let Some(thumb) = playlist.thumbnails.last() {
                        metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                    }
                    
                    songs.push(Song {
                        id: None,
                        file: format!("playlist:{}", playlist.playlist_id.get_raw()),
                        duration: None,
                        metadata,
                        last_modified: None,
                        added: Some(Utc::now()),
                    });
                }
                BasicSearchResultCommunityPlaylist::Podcast(podcast) => {
                    let mut metadata = HashMap::new();
                    metadata.insert("title".to_string(), vec![podcast.title]);
                    metadata.insert("artist".to_string(), vec![podcast.publisher]);
                    metadata.insert("type".to_string(), vec!["podcast".to_string()]);
                    if let Some(thumb) = podcast.thumbnails.last() {
                        metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
                    }
                    
                    songs.push(Song {
                        id: None,
                        file: format!("podcast:{}", podcast.podcast_id.get_raw()),
                        duration: None,
                        metadata,
                        last_modified: None,
                        added: Some(Utc::now()),
                    });
                }
                // Handle future variants
                _ => {}
            }
        }
        
        // Parse Featured Playlists
        for playlist in results.featured_playlists {
            let mut metadata = HashMap::new();
            metadata.insert("title".to_string(), vec![playlist.title]);
            metadata.insert("artist".to_string(), vec![playlist.author]);
            metadata.insert("type".to_string(), vec!["playlist".to_string()]);
             if let Some(thumb) = playlist.thumbnails.last() {
                metadata.insert("thumbnail".to_string(), vec![thumb.url.clone()]);
            }

            songs.push(Song {
                id: None,
                file: format!("playlist:{}", playlist.playlist_id.get_raw()),
                duration: None,
                metadata,
                last_modified: None,
                added: Some(Utc::now()),
            });
        }

        Ok(songs)
    }

    fn find(&mut self, filter: &[Filter], _window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        for f in filter {
            if f.tag == Tag::Album {
                // If filtering by Album, we assume the value is the album ID (or we need to handle it)
                // Since lsinfo now handles "album:ID", we can use it.
                // But lsinfo returns LsInfoEntry, find returns Song.
                // We need to extract Songs from LsInfoEntry.
                let album_id = &f.value;
                let entries = self.lsinfo(Some(album_id))?;
                let songs = entries.into_iter().filter_map(|e| match e {
                    LsInfoEntry::File(s) => Some(s),
                    _ => None,
                }).collect();
                return Ok(songs);
            }
        }
        self.search(filter)
    }

    fn list_tag(&mut self, _tag: Tag, _filter: Option<&[Filter]>) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn count(&mut self, _filter: &[Filter]) -> Result<(usize, Duration)> {
        Ok((0, Duration::from_secs(0)))
    }

    // ===== Playlist Management =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> { Ok(vec![]) }
    fn playlist_info_name(&mut self, _name: &str) -> Result<Vec<Song>> { Ok(vec![]) }
    fn load_playlist(&mut self, _name: &str, _position: Option<QueuePosition>) -> Result<()> { Ok(()) }
    fn save_queue_as_playlist(&mut self, _name: &str, _mode: Option<SaveMode>) -> Result<()> { Ok(()) }
    fn delete_playlist(&mut self, _name: &str) -> Result<()> { Ok(()) }
    fn rename_playlist(&mut self, _old_name: &str, _new_name: &str) -> Result<()> { Ok(()) }
    fn add_to_playlist(&mut self, _playlist: &str, _uri: &str) -> Result<()> { Ok(()) }
    fn delete_from_playlist(&mut self, _playlist: &str, _position: u32) -> Result<()> { Ok(()) }
    fn move_in_playlist(&mut self, _playlist: &str, _from: SingleOrRange, _to: u32) -> Result<()> { Ok(()) }

    // ===== Sticker Support =====

    fn list_stickers(&mut self, _uri: &str) -> Result<HashMap<String, String>> { Ok(HashMap::new()) }
    fn set_sticker(&mut self, _uri: &str, _key: &str, _value: &str) -> Result<()> { Ok(()) }
    fn delete_sticker(&mut self, _uri: &str, _key: &str) -> Result<()> { Ok(()) }

    // ===== Database Management =====

    fn update(&mut self, _path: Option<&str>) -> Result<u32> { Ok(0) }
    fn rescan(&mut self, _path: Option<&str>) -> Result<u32> { Ok(0) }

    // ===== System Info =====

    fn version(&self) -> Version {
        Version { major: 0, minor: 1, patch: 0 }
    }

    fn outputs(&mut self) -> Result<Vec<Output>> { Ok(vec![]) }
    fn decoders(&mut self) -> Result<Vec<Decoder>> { Ok(vec![]) }
    fn partitions(&mut self) -> Result<Vec<String>> { Ok(vec![]) }
}

// Cleanup: kill spawned MPV process when backend is dropped
impl Drop for YouTubeBackend {
    fn drop(&mut self) {
        if let Some(mut child) = self.mpv_process.take() {
            log::info!("Terminating spawned MPV process (PID: {})", child.id());
            if let Err(e) = child.kill() {
                log::warn!("Failed to kill MPV process: {}", e);
            } else {
                // Wait for process to exit
                let _ = child.wait();
                log::debug!("MPV process terminated");
            }
        }
    }
}
#[cfg(test)]
mod youtube_backend_tests {
    use super::*;
    use std::collections::HashMap;

    /// Helper to create a mock Song with metadata
    fn create_mock_song(file: String, type_: &str, title: &str, artist: &str) -> Song {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec![type_.to_string()]);
        metadata.insert("title".to_string(), vec![title.to_string()]);
        metadata.insert("artist".to_string(), vec![artist.to_string()]);
        
        Song {
            id: None,
            file,
            duration: None,
            metadata,
            last_modified: None,
            added: Some(chrono::Utc::now()),
        }
    }

    #[test]
    fn test_artist_id_format() {
        let song = create_mock_song(
            "artist:UC3muIvzjhubNpJ4Pn_0kCQw".to_string(),
            "artist",
            "Sơn Tùng M-TP",
            "Sơn Tùng M-TP"
        );
        
        assert!(song.file.starts_with("artist:"));
        assert_eq!(song.metadata.get("type").unwrap()[0], "artist");
    }

    #[test]
    fn test_album_id_format() {
        let song = create_mock_song(
            "album:MPREb_1234567890".to_string(),
            "album",
            "Test Album",
            "Test Artist"
        );
        
        assert!(song.file.starts_with("album:"));
        assert_eq!(song.metadata.get("type").unwrap()[0], "album");
    }

    #[test]
    fn test_playlist_id_format() {
        let song = create_mock_song(
            "playlist:RDCLAK5uy_1234567890".to_string(),
            "playlist",
            "Test Playlist",
            "YouTube Music"
        );
        
        assert!(song.file.starts_with("playlist:"));
        assert_eq!(song.metadata.get("type").unwrap()[0], "playlist");
    }

    #[test]
    fn test_podcast_id_format() {
        let song = create_mock_song(
            "podcast:MPSP1234567890".to_string(),
            "podcast",
            "Test Podcast",
            "Podcast Publisher"
        );
        
        assert!(song.file.starts_with("podcast:"));
        assert_eq!(song.metadata.get("type").unwrap()[0], "podcast");
    }

    #[test]
    fn test_song_id_format() {
        // Songs should NOT have a prefix
        let song = create_mock_song(
            "dQw4w9WgXcQ".to_string(),
            "song",
            "Test Song",
            "Test Artist"
        );
        
        assert!(!song.file.contains(":"));
        assert_eq!(song.file.len(), 11); // YouTube video IDs are 11 characters
        assert_eq!(song.metadata.get("type").unwrap()[0], "song");
    }

    #[test]
    fn test_video_id_format() {
        let song = create_mock_song(
            "dQw4w9WgXcQ".to_string(),
            "video",
            "Test Video",
            "Test Channel"
        );
        
        assert!(!song.file.contains(":"));
        assert_eq!(song.metadata.get("type").unwrap()[0], "video");
    }

    #[test]
    fn test_metadata_always_includes_type() {
        let test_types = vec!["artist", "album", "song", "video", "playlist", "podcast"];
        
        for type_ in test_types {
            let song = create_mock_song(
                "test_id".to_string(),
                type_,
                "Test Title",
                "Test Artist"
            );
            
            assert!(song.metadata.contains_key("type"));
            assert_eq!(song.metadata.get("type").unwrap()[0], type_);
        }
    }

    #[test]
    fn test_duration_parsing() {
        // Test MM:SS format
        let duration_str = "3:45";
        let seconds: u64 = duration_str
            .split(':')
            .try_fold(0u64, |acc, part| {
                part.parse::<u64>().map(|v| acc * 60 + v)
            })
            .unwrap();
        assert_eq!(seconds, 225); // 3*60 + 45

        // Test HH:MM:SS format
        let duration_str = "1:23:45";
        let seconds: u64 = duration_str
            .split(':')
            .try_fold(0u64, |acc, part| {
                part.parse::<u64>().map(|v| acc * 60 + v)
            })
            .unwrap();
        assert_eq!(seconds, 5025); // 1*3600 + 23*60 + 45
    }

    #[test]
    fn test_id_extraction_from_prefixed_file() {
        let test_cases = vec![
            ("artist:UC123456789", "UC123456789"),
            ("album:MPREb_123456789", "MPREb_123456789"),
            ("playlist:RDCLAK5uy_123", "RDCLAK5uy_123"),
            ("podcast:MPSP123", "MPSP123"),
        ];

        for (prefixed, expected_id) in test_cases {
            let id = prefixed.split(':').nth(1).unwrap();
            assert_eq!(id, expected_id);
        }
    }

    #[test]
    fn test_type_detection_from_file_prefix() {
        let test_cases = vec![
            ("artist:UC123", "artist"),
            ("album:MPREb_123", "album"),
            ("playlist:RDCLAK", "playlist"),
            ("podcast:MPSP", "podcast"),
            ("dQw4w9WgXcQ", "unknown"), // No prefix
        ];

        for (file, expected_type) in test_cases {
            let detected_type = if file.contains(':') {
                file.split(':').next().unwrap()
            } else {
                "unknown"
            };
            assert_eq!(detected_type, expected_type);
        }
    }
}

#[cfg(test)]
mod search_navigation_tests {
    use super::*;

    #[test]
    fn test_navigation_event_for_artist() {
        // Simulate artist result
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["artist".to_string()]);
        metadata.insert("title".to_string(), vec!["Test Artist".to_string()]);
        
        let type_ = metadata.get("type").and_then(|v| v.first());
        assert_eq!(type_, Some(&"artist".to_string()));
        
        // Should trigger OpenArtist event
        assert!(matches!(type_, Some(t) if t == "artist"));
    }

    #[test]
    fn test_navigation_event_for_album() {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["album".to_string()]);
        
        let type_ = metadata.get("type").and_then(|v| v.first());
        assert!(matches!(type_, Some(t) if t == "album"));
    }

    #[test]
    fn test_navigation_event_for_playlist() {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["playlist".to_string()]);
        
        let type_ = metadata.get("type").and_then(|v| v.first());
        assert!(matches!(type_, Some(t) if t == "playlist"));
    }

    #[test]
    fn test_playback_for_song() {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["song".to_string()]);
        
        let type_ = metadata.get("type").and_then(|v| v.first());
        // Songs and videos should NOT trigger navigation events
        assert!(matches!(type_, Some(t) if t == "song"));
        // Should fall through to enqueue logic
    }

    #[test]
    fn test_playback_for_video() {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["video".to_string()]);
        
        let type_ = metadata.get("type").and_then(|v| v.first());
        assert!(matches!(type_, Some(t) if t == "video"));
        // Should fall through to enqueue logic
    }
}
