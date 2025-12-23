//! YouTube backend client - connects to server via IPC.
//! Implements MusicBackend trait for use in TUI.

use std::{
    collections::HashMap,
    io::{BufReader, BufWriter},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};

use super::protocol::{BrowseEntry, ServerCommand, ServerResponse, SongData, SearchItemData, framing};
use crate::{
    domain::{PlaybackState, QueuePosition, Song, Status},
    mpd::{
        commands::{
            Decoder, LsInfoEntry, Output, Playlist, SaveMode, SeekPosition, ValueChange,
            lsinfo::Dir,
            status::OnOffOneshot,
        },
        mpd_client::{Filter, SingleOrRange, Tag},
        version::Version,
    },
    backends::traits::{MusicBackend, QueueOperations},
    backends::LibraryCategory,
};

/// Convert SearchItemData to Song for UI compatibility
/// This maintains backward compatibility with the existing UI which expects Song
fn search_item_data_to_song(item: SearchItemData) -> Option<Song> {
    match item {
        SearchItemData::Song(p) => Some(song_from_playable(&p, "song")),
        SearchItemData::Video(p) => Some(song_from_playable(&p, "video")),
        SearchItemData::Artist(b) => Some(song_from_browsable(&b, "artist")),
        SearchItemData::Album(b) => Some(song_from_browsable(&b, "album")),
        SearchItemData::Playlist(b) => Some(song_from_browsable(&b, "playlist")),
        SearchItemData::Header(h) => {
            let mut song = Song::default();
            song.metadata.insert("type".into(), vec!["header".into()]);
            song.metadata.insert("title".into(), vec![h]);
            Some(song)
        }
    }
}

fn song_from_playable(p: &super::protocol::PlayableData, item_type: &str) -> Song {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("title".into(), vec![p.title.clone()]);
    metadata.insert("artist".into(), vec![p.artist.clone()]);
    metadata.insert("type".into(), vec![item_type.into()]);
    if let Some(ref album) = p.album {
        metadata.insert("album".into(), vec![album.clone()]);
    }
    if let Some(ref thumb) = p.thumbnail {
        metadata.insert("thumbnail".into(), vec![thumb.clone()]);
    }
    Song {
        id: None,
        uri: p.video_id.clone(),
        duration: p.duration_ms.map(Duration::from_millis),
        metadata,
        last_modified: None,
        added: None,
    }
}

fn song_from_browsable(b: &super::protocol::BrowsableData, item_type: &str) -> Song {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("title".into(), vec![b.title.clone()]);
    metadata.insert("type".into(), vec![item_type.into()]);
    if let Some(ref subtitle) = b.subtitle {
        metadata.insert("subtitle".into(), vec![subtitle.clone()]);
    }
    if let Some(ref thumb) = b.thumbnail {
        metadata.insert("thumbnail".into(), vec![thumb.clone()]);
    }
    Song {
        id: None,
        uri: b.browse_path.clone(),
        duration: None,
        metadata,
        last_modified: None,
        added: None,
    }
}

/// YouTube backend client - connects to YouTube daemon via Unix socket IPC.
///
/// **This is the canonical entry point for YouTube backend in the TUI.**
///
/// The actual backend logic (MPV management, API calls, queue state) lives
/// in the YouTubeServer daemon process. This proxy forwards commands via IPC.
///
/// # Architecture
///
/// ```text
/// TUI Process          Daemon Process
/// ───────────          ──────────────
/// YouTubeProxy  ──IPC──>  YouTubeServer
///                            ├─> MPV (playback)
///                            ├─> YouTube API (metadata)
///                            └─> QueueService (state)
/// ```
///
/// # Communication
///
/// - **Protocol**: Custom JSON-based protocol with length-prefixed framing
/// - **Transport**: Unix domain socket (default: `/tmp/yrmpc-yt.sock`)
/// - **Pattern**: Request-response (synchronous from proxy perspective)
///
/// # Usage
///
/// ```rust,ignore
/// let proxy = YouTubeProxy::connect(Path::new("/tmp/yrmpc-yt.sock"))?;
/// // proxy implements MusicBackend trait
/// proxy.play()?;
/// ```
///
/// # Note
///
/// This is NOT a YouTube API client. It's an IPC proxy to the daemon.
/// The daemon (YouTubeServer) handles all YouTube Music API communication.
#[derive(Debug)]
pub struct YouTubeProxy {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
    socket_path: PathBuf,
}

impl YouTubeProxy {
    /// Connect to server at given socket path
    pub fn connect(socket_path: &Path) -> Result<Self> {
        // Try to connect to existing daemon
        let stream = match UnixStream::connect(socket_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("\n❌ YouTube daemon not running!\n");
                eprintln!("Start the daemon first:");
                eprintln!("  systemctl --user start rmpcd");
                eprintln!("");
                eprintln!("Or install as a service:");
                eprintln!("  ./setup/rmpcd-install");
                eprintln!("");
                eprintln!("Or run manually:");
                eprintln!("  rmpcd");
                eprintln!("");
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        log::info!("Connected to YouTube daemon at {}", socket_path.display());

        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: BufWriter::new(stream),
            socket_path: socket_path.to_path_buf(),
        })
    }

    /// Send command and receive response
    fn request(&mut self, cmd: ServerCommand) -> Result<ServerResponse> {
        log::debug!("YouTubeClient::request - writing command");
        framing::write_message(&mut self.writer, &cmd)?;
        log::debug!("YouTubeClient::request - reading response");
        let response = framing::read_message(&mut self.reader);
        log::debug!("YouTubeClient::request - got response: {:?}", response.as_ref().map(|r| format!("{:?}", r).chars().take(100).collect::<String>()));
        response
    }

    /// Send command and expect Ok response
    fn request_ok(&mut self, cmd: ServerCommand) -> Result<()> {
        log::debug!("YouTubeClient::request_ok - sending command: {:?}", std::mem::discriminant(&cmd));
        match self.request(cmd)? {
            ServerResponse::Ok => Ok(()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    /// Ping the server
    pub fn ping(&mut self) -> Result<()> {
        match self.request(ServerCommand::Ping)? {
            ServerResponse::Pong => Ok(()),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    /// Shutdown the server
    pub fn shutdown(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Shutdown)
    }
    
    // =========================================================================
    // RICH BROWSE DETAIL METHODS
    // =========================================================================
    // These return structured detail types via IPC to the daemon
    
    /// Get detailed playlist info with tracks and metadata
    pub fn browse_playlist_details(&mut self, playlist_id: &str) -> Result<super::PlaylistDetails> {
        match self.request(ServerCommand::BrowsePlaylistDetails { playlist_id: playlist_id.to_string() })? {
            ServerResponse::PlaylistDetails(data) => Ok(data.to_details()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }
    
    /// Get detailed album info with tracks and metadata
    pub fn browse_album_details(&mut self, album_id: &str) -> Result<super::AlbumDetails> {
        match self.request(ServerCommand::BrowseAlbumDetails { album_id: album_id.to_string() })? {
            ServerResponse::AlbumDetails(data) => Ok(data.to_details()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }
    
    /// Get detailed artist info with discography
    pub fn browse_artist_details(&mut self, artist_id: &str) -> Result<super::ArtistDetails> {
        match self.request(ServerCommand::BrowseArtistDetails { artist_id: artist_id.to_string() })? {
            ServerResponse::ArtistDetails(data) => Ok(data.to_details()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    /// Try to clone the underlying stream (for idle connections)
    pub fn try_clone_stream(&self) -> Result<UnixStream> {
        // Clone from the reader's underlying stream
        self.reader.get_ref().try_clone().context("Failed to clone YouTube client stream")
    }

    /// Enter idle mode - currently no-op for YouTube backend
    /// NOTE: Blocking idle was causing 30s delays on all operations.
    /// Queue updates use optimistic UI instead.
    pub fn enter_idle(&mut self) -> Result<()> {
        // Don't send blocking Idle command - it blocks the shared socket
        Ok(())
    }

    /// Read idle response - blocks briefly then returns timeout.
    ///
    /// For YouTube backend, we don't use MPD-style idle events.
    /// However, we must block here to prevent CPU spinning.
    ///
    /// The sleep duration controls the polling interval for request processing:
    /// - 100ms = 10 cycles/sec, ~0.5% CPU overhead, <100ms search latency
    ///
    /// Returns MpdError::TimedOut so the idle thread breaks its inner loop
    /// and cycles through the outer loop, giving the request thread a chance
    /// to acquire the client and process pending requests (like search).
    pub fn read_response(&mut self) -> Result<Vec<crate::mpd::commands::IdleEvent>> {
        // Sleep to prevent CPU spinning and control polling frequency
        std::thread::sleep(Duration::from_millis(100));

        // Return timeout error so the idle thread yields to request thread
        Err(anyhow::Error::new(crate::mpd::errors::MpdError::TimedOut(
            "YouTube backend idle timeout".into()
        )))
    }

    /// Reconnect to server after connection loss
    ///
    /// This is called by core/client.rs when the connection drops (e.g., "Broken pipe").
    /// The outer reconnection loop will retry this until it succeeds or the daemon
    /// becomes available again.
    pub fn reconnect(&mut self) -> Result<()> {
        log::info!("Attempting to reconnect to YouTube daemon at {}", self.socket_path.display());

        let stream = UnixStream::connect(&self.socket_path)
            .context("Failed to reconnect to YouTube daemon")?;

        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        self.reader = BufReader::new(stream.try_clone()?);
        self.writer = BufWriter::new(stream);

        log::info!("Successfully reconnected to YouTube daemon at {}", self.socket_path.display());
        Ok(())
    }

    /// Play song at specific position (0-indexed)
    pub fn play_pos(&mut self, pos: usize) -> Result<()> {
        self.request_ok(ServerCommand::PlayPos(pos))
    }

    /// Add song with full metadata (preferred over add for preserving metadata)
    pub fn add_song(&mut self, song: &Song, position: Option<u32>) -> Result<()> {
        let song_data = SongData::from(song.clone());
        log::debug!("YouTubeClient::add_song sending AddSong command: file={}, title={:?}", 
            song_data.file, song_data.title);
        let result = self.request_ok(ServerCommand::AddSong { song: song_data, position });
        log::debug!("YouTubeClient::add_song result: {:?}", result.as_ref().map(|_| "Ok"));
        result
    }
}

// === QueueOperations Implementation ===

impl QueueOperations for YouTubeProxy {
    fn enqueue(&mut self, song: &Song, position: Option<QueuePosition>) -> Result<()> {
        let song_data = SongData::from(song.clone());
        let pos = position.and_then(|p| match p {
            QueuePosition::Absolute(n) => Some(n as u32),
            _ => None,
        });
        self.request_ok(ServerCommand::AddSong { song: song_data, position: pos })
    }

    fn dequeue(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::DeleteId(id))
    }

    fn reorder(&mut self, from_id: u32, to_id: u32) -> Result<()> {
        self.request_ok(ServerCommand::MoveId { from: from_id, to: to_id })
    }

    fn clear_queue(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Clear)
    }

    fn play_by_id(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::PlayId(id))
    }
}

impl MusicBackend for YouTubeProxy {
    fn backend_name(&self) -> &'static str {
        "YouTube"
    }

    fn capabilities(&self) -> &'static [crate::backends::BackendCapability] {
        use crate::backends::BackendCapability::*;
        &[RichMetadata]
    }

    // supports() uses default implementation from trait

    // === Playback Control ===

    fn play(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Play)
    }

    fn pause(&mut self, state: bool) -> Result<()> {
        if state {
            self.request_ok(ServerCommand::Pause)
        } else {
            self.request_ok(ServerCommand::Play)
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Stop)
    }

    fn next(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Next)
    }

    fn previous(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Previous)
    }

    fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        match position {
            SeekPosition::Absolute(secs) => {
                self.request_ok(ServerCommand::SeekAbsolute(secs as f64))
            }
            SeekPosition::Relative(secs) => {
                self.request_ok(ServerCommand::SeekRelative(secs as f64))
            }
        }
    }

    // === Status ===

    fn get_status(&mut self) -> Result<Status> {
        match self.request(ServerCommand::GetStatus)? {
            ServerResponse::Status(s) => Ok(s.to_status()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        match self.request(ServerCommand::GetCurrentSong)? {
            ServerResponse::Song(s) => Ok(s.map(|sd| sd.to_song())),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        match self.request(ServerCommand::GetPlaylist)? {
            ServerResponse::Playlist(songs) => Ok(songs.into_iter().map(|sd| sd.to_song()).collect()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    // === Queue Management ===

    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        let pos = position.map(|p| match p {
            QueuePosition::Absolute(n) => n as u32,
            QueuePosition::Relative(n) => n as u32,
            QueuePosition::End => u32::MAX, // Append to end
            QueuePosition::Next => 0,       // After current
        });
        self.request_ok(ServerCommand::Add { uri: uri.to_string(), position: pos })
    }

    fn delete_id(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::DeleteId(id))
    }

    fn clear(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Clear)
    }

    fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.request_ok(ServerCommand::MoveId { from, to })
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::PlayId(id))
    }

    // === Volume ===

    fn volume(&mut self) -> Result<u8> {
        match self.request(ServerCommand::GetVolume)? {
            ServerResponse::Volume(v) => Ok(v),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        match volume {
            ValueChange::Set(v) => self.request_ok(ServerCommand::SetVolume(v as u8)),
            ValueChange::Increase(d) => self.request_ok(ServerCommand::AdjustVolume(d as i8)),
            ValueChange::Decrease(d) => self.request_ok(ServerCommand::AdjustVolume(-(d as i8))),
        }
    }

    // === Search/Browse ===

    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>> {
        let query = filter
            .iter()
            .find_map(|f| if !f.value.is_empty() { Some(f.value.as_ref()) } else { None })
            .unwrap_or("");

        if query.is_empty() {
            return Ok(vec![]);
        }

        match self.request(ServerCommand::Search { query: query.to_string() })? {
            ServerResponse::SearchResults(items) => {
                // Convert SearchItemData to Song for UI compatibility
                Ok(items.into_iter().filter_map(|item| {
                    search_item_data_to_song(item)
                }).collect())
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn find(&mut self, filter: &[Filter], _window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        self.search(filter)
    }

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        let path = path.unwrap_or("");
        if path.is_empty() {
            return Ok(vec![]);
        }

        match self.request(ServerCommand::Browse { path: path.to_string() })? {
            ServerResponse::BrowseResults(entries) => {
                Ok(entries
                    .into_iter()
                    .map(|e| match e {
                        BrowseEntry::Dir { name, path } => LsInfoEntry::Dir(Dir {
                            name,
                            full_path: path,
                            last_modified: chrono::Utc::now(),
                        }),
                        BrowseEntry::File(sd) => LsInfoEntry::File(sd.to_song()),
                    })
                    .collect())
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    // === Playback Options ===

    fn repeat(&mut self, repeat: bool) -> Result<()> {
        // MPD repeat toggles between repeat-all and off
        // For YouTube, we map: repeat=true → "all", repeat=false → "off"
        let mode = if repeat { "all" } else { "off" };
        self.request_ok(ServerCommand::SetRepeat(mode.to_string()))
    }

    fn random(&mut self, random: bool) -> Result<()> {
        self.request_ok(ServerCommand::SetShuffle(random))
    }

    fn single(&mut self, single: OnOffOneshot) -> Result<()> {
        // MPD uses 'single' for Repeat One mode
        // When single=On, we want Repeat One
        // When single=Off, we revert to current repeat mode (all or off)
        match single {
            OnOffOneshot::On | OnOffOneshot::Oneshot => {
                // Enable Repeat One
                self.request_ok(ServerCommand::SetRepeat("one".to_string()))
            }
            OnOffOneshot::Off => {
                // Disable Repeat One - check if repeat was enabled to determine target
                // For simplicity, just set to "off" - user can re-enable repeat if needed
                self.request_ok(ServerCommand::SetRepeat("off".to_string()))
            }
        }
    }

    fn consume(&mut self, _consume: OnOffOneshot) -> Result<()> {
        // Uses default no-op from trait
        Ok(())
    }

    // crossfade, shuffle use default no-op implementations from trait

    // === Library ===

    // list_all, list_tag, count use default implementations from trait

    fn get_library(
        &mut self,
        _category: LibraryCategory,
    ) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn get_search_suggestions(&mut self, query: String) -> Result<Vec<String>> {
        if query.is_empty() {
            return Ok(vec![]);
        }
        match self.request(ServerCommand::GetSearchSuggestions { query })? {
            ServerResponse::Suggestions(suggestions) => Ok(suggestions),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    // Playlist management, stickers, database management, outputs, decoders, partitions
    // all use default implementations from trait

    fn version(&self) -> Version {
        Version { major: 0, minor: 1, patch: 0 }
    }
}

//=============================================================================
// API TRAIT IMPLEMENTATION
//=============================================================================
//
// These traits provide a clean, MPD-free interface for the TUI.
// They wrap the existing MusicBackend methods with simpler types.

use crate::backends::api::{self, Item, SearchQuery, SearchResults, BrowseResult, Capability, InsertAt, AfterAdd};

impl api::Playback for YouTubeProxy {
    fn play(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Play)
    }

    fn pause(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Pause)
    }

    fn stop(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Stop)
    }

    fn next(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Next)
    }

    fn previous(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Previous)
    }

    fn seek(&mut self, position: std::time::Duration) -> Result<()> {
        self.request_ok(ServerCommand::SeekAbsolute(position.as_secs_f64()))
    }

    fn seek_relative(&mut self, delta_secs: i64) -> Result<()> {
        self.request_ok(ServerCommand::SeekRelative(delta_secs as f64))
    }

    fn status(&mut self) -> Result<api::Status> {
        match self.request(ServerCommand::GetStatus)? {
            ServerResponse::Status(s) => {
                let domain_status = s.to_status();
                Ok(api::Status {
                    state: domain_status.state.into(),
                    position: domain_status.elapsed,
                    duration: domain_status.duration,
                    volume: domain_status.volume,
                    repeat: match (domain_status.repeat, domain_status.single) {
                        (true, crate::domain::status::OnOffOneshot::On) => api::Repeat::One,
                        (true, _) => api::Repeat::All,
                        (false, _) => api::Repeat::Off,
                    },
                    shuffle: domain_status.random,
                })
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }
}

impl api::Queue for YouTubeProxy {
    fn add(&mut self, items: &[Item], at: InsertAt, after: AfterAdd) -> Result<()> {
        // Handle Replace mode - clear first
        if at == InsertAt::Replace {
            self.request_ok(ServerCommand::Clear)?;
        }

        // Calculate starting position
        let start_pos = match at {
            InsertAt::End | InsertAt::Replace => None,
            InsertAt::Next => {
                // Get current position and insert after it
                if let ServerResponse::Status(s) = self.request(ServerCommand::GetStatus)? {
                    s.current_pos.map(|p| p + 1)
                } else {
                    None
                }
            }
            InsertAt::Position(p) => Some(p),
        };

        // Add each item
        for (i, item) in items.iter().enumerate() {
            let pos = start_pos.map(|p| p + i as u32);
            let song_data = SongData {
                id: item.queue_id,
                file: item.id.clone(),
                title: Some(item.title.clone()),
                artist: item.subtitle.clone(),
                album: None,
                duration_ms: item.duration.map(|d| d.as_millis() as u64),
                thumbnail: item.thumbnail.clone(),
                item_type: Some(match item.content_type {
                    api::ContentType::Track => "song",
                    api::ContentType::Album => "album",
                    api::ContentType::Artist => "artist",
                    api::ContentType::Playlist => "playlist",
                    _ => "song",
                }.to_string()),
            };
            self.request_ok(ServerCommand::AddSong { song: song_data, position: pos })?;
        }

        // Handle autoplay
        match after {
            AfterAdd::Nothing => {}
            AfterAdd::PlayFirst => {
                // Play the first added item
                if let Some(pos) = start_pos {
                    self.request_ok(ServerCommand::PlayPos(pos as usize))?;
                } else if !items.is_empty() {
                    // Added at end, play last position
                    if let ServerResponse::Status(s) = self.request(ServerCommand::GetStatus)? {
                        let play_pos = s.playlist_length.saturating_sub(items.len() as u32);
                        self.request_ok(ServerCommand::PlayPos(play_pos as usize))?;
                    }
                }
            }
            AfterAdd::PlayIndex(idx) => {
                if idx < items.len() {
                    if let Some(pos) = start_pos {
                        self.request_ok(ServerCommand::PlayPos((pos as usize) + idx))?;
                    }
                }
            }
        }

        Ok(())
    }

    fn remove(&mut self, queue_ids: &[u32]) -> Result<()> {
        for id in queue_ids {
            self.request_ok(ServerCommand::DeleteId(*id))?;
        }
        Ok(())
    }

    fn list(&mut self) -> Result<Vec<Item>> {
        match self.request(ServerCommand::GetPlaylist)? {
            ServerResponse::Playlist(songs) => {
                Ok(songs.into_iter().map(|sd| {
                    Item {
                        id: sd.file.clone(),
                        content_type: api::ContentType::Track,
                        title: sd.title.unwrap_or_else(|| sd.file.clone()),
                        subtitle: sd.artist,
                        thumbnail: sd.thumbnail,
                        duration: sd.duration_ms.map(std::time::Duration::from_millis),
                        queue_id: sd.id,
                    }
                }).collect())
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()> {
        // Move each item to the target position
        // Note: This is a simplification - proper bulk move would need daemon support
        for (i, id) in queue_ids.iter().enumerate() {
            self.request_ok(ServerCommand::MoveId { 
                from: *id, 
                to: to_position + i as u32 
            })?;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Clear)
    }

    fn play_id(&mut self, queue_id: u32) -> Result<()> {
        self.request_ok(ServerCommand::PlayId(queue_id))
    }

    fn set_repeat(&mut self, mode: api::Repeat) -> Result<()> {
        let mode_str = match mode {
            api::Repeat::Off => "off",
            api::Repeat::All => "all",
            api::Repeat::One => "one",
        };
        self.request_ok(ServerCommand::SetRepeat(mode_str.to_string()))
    }

    fn set_shuffle(&mut self, enabled: bool) -> Result<()> {
        self.request_ok(ServerCommand::SetShuffle(enabled))
    }
}

impl api::Discovery for YouTubeProxy {
    fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        if query.text.is_empty() {
            return Ok(SearchResults::default());
        }
        
        match self.request(ServerCommand::Search { query: query.text })? {
            ServerResponse::SearchResults(items) => {
                let items = items.into_iter().filter_map(|item| {
                    search_item_data_to_song(item).map(|song| Item::from(&song))
                }).collect();
                Ok(SearchResults { items })
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        if path.is_empty() {
            return Ok(BrowseResult {
                path: path.to_string(),
                items: vec![],
                parent: None,
            });
        }

        match self.request(ServerCommand::Browse { path: path.to_string() })? {
            ServerResponse::BrowseResults(entries) => {
                let items = entries.into_iter().map(|e| match e {
                    BrowseEntry::Dir { name, path } => Item {
                        id: path,
                        content_type: api::ContentType::Directory,
                        title: name,
                        subtitle: None,
                        thumbnail: None,
                        duration: None,
                        queue_id: None,
                    },
                    BrowseEntry::File(sd) => Item {
                        id: sd.file.clone(),
                        content_type: api::ContentType::Track,
                        title: sd.title.unwrap_or_else(|| sd.file.clone()),
                        subtitle: sd.artist,
                        thumbnail: sd.thumbnail,
                        duration: sd.duration_ms.map(std::time::Duration::from_millis),
                        queue_id: sd.id,
                    },
                }).collect();

                Ok(BrowseResult {
                    path: path.to_string(),
                    items,
                    parent: path.rsplit_once('/').map(|(p, _)| p.to_string()),
                })
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn suggestions(&mut self, partial: &str) -> Result<Vec<String>> {
        if partial.is_empty() {
            return Ok(vec![]);
        }
        match self.request(ServerCommand::GetSearchSuggestions { query: partial.to_string() })? {
            ServerResponse::Suggestions(s) => Ok(s),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn details(&mut self, item: &Item) -> Result<crate::domain::ContentDetails> {
        use crate::domain::{ContentDetails, AlbumDetails, ArtistDetails, PlaylistDetails};
        use crate::domain::{ArtistRef, AlbumRef, PlaylistRef};
        
        match item.content_type {
            api::ContentType::Album => {
                let yt_album = self.browse_album_details(&item.id)?;
                Ok(ContentDetails::Album(AlbumDetails {
                    id: yt_album.id,
                    title: yt_album.title,
                    artist: ArtistRef {
                        id: yt_album.artist.id,
                        name: yt_album.artist.name,
                        thumbnail: yt_album.artist.thumbnail,
                    },
                    year: yt_album.year,
                    description: None, // YouTube albums may not have description in current impl
                    thumbnail: yt_album.thumbnail,
                    tracks: yt_album.tracks,
                    more_by_artist: yt_album.more_by_artist.into_iter().map(|a| AlbumRef {
                        id: a.id,
                        title: a.title,
                        year: a.year,
                        thumbnail: a.thumbnail,
                    }).collect(),
                }))
            }
            api::ContentType::Artist => {
                let yt_artist = self.browse_artist_details(&item.id)?;
                Ok(ContentDetails::Artist(ArtistDetails {
                    id: yt_artist.id,
                    name: yt_artist.name,
                    subscribers: yt_artist.subscribers,
                    description: yt_artist.description,
                    thumbnail: yt_artist.thumbnail,
                    top_songs: yt_artist.top_songs,
                    albums: yt_artist.albums.into_iter().map(|a| AlbumRef {
                        id: a.id,
                        title: a.title,
                        year: a.year,
                        thumbnail: a.thumbnail,
                    }).collect(),
                    singles: yt_artist.singles.into_iter().map(|a| AlbumRef {
                        id: a.id,
                        title: a.title,
                        year: a.year,
                        thumbnail: a.thumbnail,
                    }).collect(),
                    related_artists: yt_artist.related_artists.into_iter().map(|a| ArtistRef {
                        id: a.id,
                        name: a.name,
                        thumbnail: a.thumbnail,
                    }).collect(),
                }))
            }
            api::ContentType::Playlist => {
                let yt_playlist = self.browse_playlist_details(&item.id)?;
                Ok(ContentDetails::Playlist(PlaylistDetails {
                    id: yt_playlist.id,
                    title: yt_playlist.title,
                    author: yt_playlist.artist,
                    year: yt_playlist.year,
                    description: None, // Not currently fetched
                    thumbnail: yt_playlist.thumbnail,
                    track_count: yt_playlist.track_count,
                    duration_text: yt_playlist.duration_text,
                    tracks: yt_playlist.tracks,
                    featured_artists: yt_playlist.featured_artists.into_iter().map(|a| ArtistRef {
                        id: a.id,
                        name: a.name,
                        thumbnail: a.thumbnail,
                    }).collect(),
                    related_playlists: yt_playlist.related_playlists.into_iter().map(|p| PlaylistRef {
                        id: p.id,
                        title: p.title,
                        subtitle: p.subtitle,
                        thumbnail: p.thumbnail,
                    }).collect(),
                }))
            }
            other => Err(anyhow!("Cannot get details for content type: {:?}", other)),
        }
    }

    // resolve() uses default implementation - track returns itself
}

impl api::Volume for YouTubeProxy {
    fn get(&mut self) -> Result<u8> {
        match self.request(ServerCommand::GetVolume)? {
            ServerResponse::Volume(v) => Ok(v),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn set(&mut self, volume: u8) -> Result<()> {
        self.request_ok(ServerCommand::SetVolume(volume))
    }
}

impl api::Backend for YouTubeProxy {
    fn name(&self) -> &'static str {
        "YouTube"
    }

    fn capabilities(&self) -> &[Capability] {
        &[Capability::RichMetadata]
    }
}

#[cfg(test)]
mod tests {
    // Integration tests would start a server and connect client
    // For unit tests, we'd mock the socket connection
}
