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

use super::protocol::{BrowseEntry, ServerCommand, ServerResponse, SongData, framing};
use crate::{
    backends::{
        LibraryCategory,
        api::{Item, SearchSection},
        traits::{MusicBackend, QueueOperations},
    },
    domain::{MediaItem, PlaybackState, QueuePosition, Song, Status},
    mpd::{
        commands::{
            Decoder,
            LsInfoEntry,
            Output,
            Playlist,
            SaveMode,
            SeekPosition,
            ValueChange,
            lsinfo::Dir,
            status::OnOffOneshot,
        },
        mpd_client::{Filter, SingleOrRange, Tag},
        version::Version,
    },
};

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

/// Parse a flat list of MediaItems (with Header markers) into structured
/// sections.
///
/// This implements the Section-as-Container pattern from
/// ADR-section-as-container.md: Headers in the protocol are converted to
/// section containers, not kept as markers.
fn parse_media_items_to_sections(items: Vec<MediaItem>) -> Vec<SearchSection> {
    let mut sections: Vec<SearchSection> = Vec::new();
    let mut current_section: Option<SearchSection> = None;

    for item in items {
        match item {
            MediaItem::Header { ref title } => {
                // Save previous section if exists
                if let Some(section) = current_section.take() {
                    if !section.items.is_empty() {
                        sections.push(section);
                    }
                }
                // Start new section
                let key = header_title_to_key(title);
                current_section = Some(SearchSection::new(key, title.clone(), Vec::new()));
            }
            _ => {
                // Add item to current section (or create unknown section)
                let section = current_section
                    .get_or_insert_with(|| SearchSection::new("unknown", "Unknown", Vec::new()));
                section.items.push(Item::from(item));
            }
        }
    }

    // Don't forget the last section
    if let Some(section) = current_section {
        if !section.items.is_empty() {
            sections.push(section);
        }
    }

    sections
}

/// Convert header display title to config key
fn header_title_to_key(title: &str) -> String {
    match title.to_lowercase().as_str() {
        "top result" | "top results" => "top_results".to_string(),
        "songs" => "songs".to_string(),
        "artists" => "artists".to_string(),
        "albums" => "albums".to_string(),
        "playlists" | "featured playlists" | "community playlists" => "playlists".to_string(),
        "videos" => "videos".to_string(),
        other => other.to_lowercase().replace(' ', "_"),
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
        log::debug!(
            "YouTubeClient::request - got response: {:?}",
            response.as_ref().map(|r| format!("{:?}", r).chars().take(100).collect::<String>())
        );
        response
    }

    /// Send command and expect Ok response
    fn request_ok(&mut self, cmd: ServerCommand) -> Result<()> {
        log::debug!(
            "YouTubeClient::request_ok - sending command: {:?}",
            std::mem::discriminant(&cmd)
        );
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
        match self.request(ServerCommand::BrowsePlaylistDetails {
            playlist_id: playlist_id.to_string(),
        })? {
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
        match self
            .request(ServerCommand::BrowseArtistDetails { artist_id: artist_id.to_string() })?
        {
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
            "YouTube backend idle timeout".into(),
        )))
    }

    /// Reconnect to server after connection loss
    ///
    /// This is called by core/client.rs when the connection drops (e.g.,
    /// "Broken pipe"). The outer reconnection loop will retry this until it
    /// succeeds or the daemon becomes available again.
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
        log::debug!(
            "YouTubeClient::add_song sending AddSong command: file={}, title={:?}",
            song_data.file,
            song_data.title
        );
        let result = self.request_ok(ServerCommand::AddSong { song: song_data, position });
        log::debug!("YouTubeClient::add_song result: {:?}", result.as_ref().map(|_| "Ok"));
        result
    }

    /// Get library contents for a category (YouTube-specific)
    pub fn get_library_internal(&mut self, _category: LibraryCategory) -> Result<Vec<LsInfoEntry>> {
        // YouTube library categories are not implemented yet
        Ok(vec![])
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

    fn capabilities(&self) -> &'static [crate::backends::api::Capability] {
        use crate::backends::api::Capability::*;
        &[RichMetadata, SearchSuggestions, Radio]
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
            ServerResponse::Playlist(songs) => {
                Ok(songs.into_iter().map(|sd| sd.to_song()).collect())
            }
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
                // [DIAG-IMG] Log first item to verify thumbnails in received IPC data
                if !items.is_empty() {
                    log::info!(
                        "[DIAG-IMG] client.search: received {} MediaItem entries, first = {:?}",
                        items.len(),
                        &items[0]
                    );
                }
                // Convert MediaItem to Song for legacy UI compatibility (skip headers)
                Ok(items
                    .into_iter()
                    .filter_map(|item| {
                        use crate::domain::MediaItem;
                        match &item {
                            MediaItem::Header { .. } => None, // Skip headers in legacy search
                            _ => {
                                let song = Song::from(item.clone());
                                log::info!(
                                    "[DIAG-IMG] client.search: converted '{}' thumbnail={:?}",
                                    song.metadata
                                        .get("title")
                                        .and_then(|v| v.first())
                                        .unwrap_or(&"?".to_string()),
                                    song.metadata.get("thumbnail")
                                );
                                Some(song)
                            }
                        }
                    })
                    .collect())
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
            ServerResponse::BrowseResults(entries) => Ok(entries
                .into_iter()
                .map(|e| match e {
                    BrowseEntry::Dir { name, path } => LsInfoEntry::Dir(Dir {
                        name,
                        full_path: path,
                        last_modified: chrono::Utc::now(),
                    }),
                    BrowseEntry::File(sd) => LsInfoEntry::File(sd.to_song()),
                })
                .collect()),
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

    fn get_library(&mut self, _category: LibraryCategory) -> Result<Vec<LsInfoEntry>> {
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

    // Playlist management, stickers, database management, outputs, decoders,
    // partitions all use default implementations from trait

    fn version(&self) -> Version {
        Version { major: 0, minor: 1, patch: 0 }
    }
}

//=============================================================================
// API TRAIT IMPLEMENTATION
//=============================================================================
// These traits provide a clean, MPD-free interface for the TUI.
// They wrap the existing MusicBackend methods with simpler types.

use crate::backends::api::{
    self,
    AfterAdd,
    BrowseResult,
    Capability,
    InsertAt,
    SearchQuery,
    SearchResults,
};

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
                    crossfade: 0,   // YouTube backend doesn't support crossfade
                    gapless: false, // YouTube backend doesn't support gapless
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
                item_type: Some(
                    match item.content_type {
                        api::ContentType::Track => "song",
                        api::ContentType::Album => "album",
                        api::ContentType::Artist => "artist",
                        api::ContentType::Playlist => "playlist",
                        _ => "song",
                    }
                    .to_string(),
                ),
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
            ServerResponse::Playlist(songs) => Ok(songs
                .into_iter()
                .map(|sd| Item {
                    id: sd.file.clone(),
                    content_type: api::ContentType::Track,
                    title: sd.title.unwrap_or_else(|| sd.file.clone()),
                    subtitle: sd.artist,
                    thumbnail: sd.thumbnail,
                    duration: sd.duration_ms.map(std::time::Duration::from_millis),
                    queue_id: sd.id,
                })
                .collect()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()> {
        // Move each item to the target position
        // Note: This is a simplification - proper bulk move would need daemon support
        for (i, id) in queue_ids.iter().enumerate() {
            self.request_ok(ServerCommand::MoveId { from: *id, to: to_position + i as u32 })?;
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
                // Parse flat MediaItem list (with Header markers) into structured sections
                // This implements the Section-as-Container pattern from
                // ADR-section-as-container.md
                let sections = parse_media_items_to_sections(items);
                let mut results = SearchResults::default();
                for section in sections {
                    results.add_section(section);
                }
                Ok(results)
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        if path.is_empty() {
            return Ok(BrowseResult { path: path.to_string(), items: vec![], parent: None });
        }

        match self.request(ServerCommand::Browse { path: path.to_string() })? {
            ServerResponse::BrowseResults(entries) => {
                let items = entries
                    .into_iter()
                    .map(|e| match e {
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
                    })
                    .collect();

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

    fn details(&mut self, item: &Item) -> Result<crate::domain::content::ContentDetails> {
        use crate::domain::content::{
            Action,
            AlbumContent,
            ArtistContent,
            ContentDetails,
            ContentRef,
            Extensions,
            PlaylistContent,
            Stat,
        };

        match item.content_type {
            api::ContentType::Album => {
                let yt_album = self.browse_album_details(&item.id)?;

                // Build extensions with stats and related content
                let mut extensions = Extensions::builder();

                // Add stats
                let mut stats = vec![];
                if let Some(year) = &yt_album.year {
                    if let Ok(y) = year.parse::<u16>() {
                        stats.push(Stat::year(y));
                    }
                }
                stats.push(Stat::track_count(yt_album.tracks.len()));
                extensions = extensions.stats(stats);

                // Add actions
                extensions = extensions.actions(vec![
                    Action::play(),
                    Action::shuffle(),
                    Action::add_to_queue(),
                ]);

                // Add "more by artist" section
                if !yt_album.more_by_artist.is_empty() {
                    let more_albums: Vec<ContentRef> = yt_album
                        .more_by_artist
                        .into_iter()
                        .map(|a| {
                            ContentRef::album(a.id, a.title)
                                .with_subtitle(a.year.unwrap_or_default())
                        })
                        .collect();
                    extensions = extensions
                        .more_by_artist(format!("More by {}", yt_album.artist.name), more_albums);
                }

                Ok(ContentDetails::Album(AlbumContent {
                    id: yt_album.id,
                    title: yt_album.title,
                    artist: ContentRef::artist(yt_album.artist.id, yt_album.artist.name)
                        .with_thumbnail(yt_album.artist.thumbnail.unwrap_or_default()),
                    tracks: yt_album.tracks,
                    thumbnail: yt_album.thumbnail,
                    year: yt_album.year.and_then(|y| y.parse().ok()),
                    release_type: None,
                    description: None,
                    extensions: extensions.build(),
                }))
            }
            api::ContentType::Artist => {
                let yt_artist = self.browse_artist_details(&item.id)?;

                // Build extensions
                let mut extensions = Extensions::builder();

                // Add stats
                let mut stats = vec![];
                if let Some(subs) = &yt_artist.subscribers {
                    stats.push(Stat::subscribers(subs.clone()));
                }
                extensions = extensions.stats(stats);

                // Add actions
                extensions =
                    extensions.actions(vec![Action::play(), Action::shuffle(), Action::radio()]);

                // Add albums section
                if !yt_artist.albums.is_empty() {
                    let albums: Vec<ContentRef> = yt_artist
                        .albums
                        .into_iter()
                        .map(|a| {
                            ContentRef::album(a.id, a.title)
                                .with_subtitle(a.year.unwrap_or_default())
                        })
                        .collect();
                    extensions = extensions.albums("Albums", albums);
                }

                // Add singles section
                if !yt_artist.singles.is_empty() {
                    let singles: Vec<ContentRef> = yt_artist
                        .singles
                        .into_iter()
                        .map(|a| {
                            ContentRef::album(a.id, a.title)
                                .with_subtitle(a.year.unwrap_or_default())
                        })
                        .collect();
                    extensions = extensions.singles("Singles", singles);
                }

                // Add related artists section
                if !yt_artist.related_artists.is_empty() {
                    let related: Vec<ContentRef> = yt_artist
                        .related_artists
                        .into_iter()
                        .map(|a| ContentRef::artist(a.id, a.name))
                        .collect();
                    extensions = extensions.related_artists("Fans also like", related);
                }

                Ok(ContentDetails::Artist(ArtistContent {
                    id: yt_artist.id,
                    name: yt_artist.name,
                    top_songs: yt_artist.top_songs,
                    thumbnail: yt_artist.thumbnail,
                    bio: yt_artist.description,
                    extensions: extensions.build(),
                }))
            }
            api::ContentType::Playlist => {
                let yt_playlist = self.browse_playlist_details(&item.id)?;

                // Build extensions
                let mut extensions = Extensions::builder();

                // Add stats
                let mut stats = vec![];
                stats.push(Stat::track_count(yt_playlist.track_count));
                if let Some(duration) = &yt_playlist.duration_text {
                    stats.push(Stat::text(
                        crate::domain::content::StatKey::Duration,
                        "Duration",
                        duration.clone(),
                    ));
                }
                extensions = extensions.stats(stats);

                // Add actions
                extensions = extensions.actions(vec![
                    Action::play(),
                    Action::shuffle(),
                    Action::add_to_queue(),
                ]);

                // Add featured artists section
                if !yt_playlist.featured_artists.is_empty() {
                    let artists: Vec<ContentRef> = yt_playlist
                        .featured_artists
                        .into_iter()
                        .map(|a| ContentRef::artist(a.id, a.name))
                        .collect();
                    extensions = extensions.featured_artists("Featured artists", artists);
                }

                // Add related playlists section
                if !yt_playlist.related_playlists.is_empty() {
                    let playlists: Vec<ContentRef> = yt_playlist
                        .related_playlists
                        .into_iter()
                        .map(|p| {
                            ContentRef::playlist(p.id, p.title)
                                .with_subtitle(p.subtitle.unwrap_or_default())
                        })
                        .collect();
                    extensions = extensions.related_playlists("Similar playlists", playlists);
                }

                Ok(ContentDetails::Playlist(PlaylistContent {
                    id: yt_playlist.id,
                    title: yt_playlist.title,
                    tracks: yt_playlist.tracks,
                    author: yt_playlist.artist.map(|name| ContentRef::new("", name)),
                    thumbnail: yt_playlist.thumbnail,
                    description: None,
                    track_count: Some(yt_playlist.track_count),
                    duration_text: yt_playlist.duration_text,
                    extensions: extensions.build(),
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

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::RichMetadata, Capability::SearchSuggestions, Capability::Radio]
    }
}

impl api::StatusQuery for YouTubeProxy {
    fn get_status(&mut self) -> anyhow::Result<crate::domain::Status> {
        match self.request(ServerCommand::GetStatus)? {
            ServerResponse::Status(s) => Ok(s.to_status()),
            ServerResponse::Error(e) => Err(anyhow::anyhow!(e)),
            other => Err(anyhow::anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn current_song(&mut self) -> anyhow::Result<Option<crate::domain::Song>> {
        match self.request(ServerCommand::GetCurrentSong)? {
            ServerResponse::Song(song) => Ok(song.map(|s| s.to_song())),
            ServerResponse::Error(e) => Err(anyhow::anyhow!(e)),
            other => Err(anyhow::anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn queue_songs(&mut self) -> anyhow::Result<Vec<crate::domain::Song>> {
        match self.request(ServerCommand::GetPlaylist)? {
            ServerResponse::Playlist(songs) => Ok(songs.into_iter().map(|s| s.to_song()).collect()),
            ServerResponse::Error(e) => Err(anyhow::anyhow!(e)),
            other => Err(anyhow::anyhow!("Unexpected response: {:?}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backends::youtube::protocol::{BrowsableData, PlayableData, SearchItemData},
        domain::display::ListItemDisplay,
    };

    // =========================================================================
    // Task-53: RED Tests for Thumbnail Preservation Bug
    // =========================================================================
    //
    // BUG CONTEXT: Thumbnails are present at daemon side (verified via logs)
    // but missing at TUI side. This test verifies the conversion functions
    // correctly preserve thumbnails.
    //
    // EXPECTED: These tests should PASS if the code is correct.
    // If they FAIL, the bug is in the conversion functions.
    // If they PASS but runtime still fails, the bug is in IPC serialization.

    #[test]
    fn song_from_playable_preserves_thumbnail() {
        // Arrange: Create PlayableData with a thumbnail
        let playable = PlayableData {
            video_id: "abc123".to_string(),
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: Some("Test Album".to_string()),
            duration_ms: Some(180000),
            thumbnail: Some("https://example.com/thumb.jpg".to_string()),
        };

        // Act: Convert to Song
        let song = song_from_playable(&playable, "song");

        // Assert: Thumbnail should be in metadata
        let thumb = song.metadata.get("thumbnail");
        assert!(
            thumb.is_some(),
            "BUG: Song metadata should contain 'thumbnail' key after conversion"
        );
        assert_eq!(
            thumb.unwrap().first().map(|s| s.as_str()),
            Some("https://example.com/thumb.jpg"),
            "BUG: Thumbnail URL should match the input"
        );

        // Also verify via ListItemDisplay trait
        assert_eq!(
            song.thumbnail_url(),
            Some("https://example.com/thumb.jpg"),
            "BUG: Song::thumbnail_url() should return the thumbnail"
        );
    }

    #[test]
    fn song_from_browsable_preserves_thumbnail() {
        // Arrange: Create BrowsableData with a thumbnail
        let browsable = BrowsableData {
            browse_id: Some("artist123".to_string()),
            browse_path: "artist:artist123".to_string(),
            title: "Test Artist".to_string(),
            subtitle: Some("1M subscribers".to_string()),
            thumbnail: Some("https://example.com/artist.jpg".to_string()),
            can_queue: false,
        };

        // Act: Convert to Song
        let song = song_from_browsable(&browsable, "artist");

        // Assert: Thumbnail should be in metadata
        let thumb = song.metadata.get("thumbnail");
        assert!(
            thumb.is_some(),
            "BUG: Song metadata should contain 'thumbnail' key for browsable items"
        );
        assert_eq!(
            thumb.unwrap().first().map(|s| s.as_str()),
            Some("https://example.com/artist.jpg"),
            "BUG: Thumbnail URL should match the input"
        );

        // Also verify via ListItemDisplay trait
        assert_eq!(
            song.thumbnail_url(),
            Some("https://example.com/artist.jpg"),
            "BUG: Song::thumbnail_url() should return the thumbnail for browsable items"
        );
    }

    // =========================================================================
    // MediaItem Conversion Tests (new architecture)
    // =========================================================================
    //
    // These tests verify that MediaItem survives JSON serialization
    // and conversion to Song for legacy compatibility.

    #[test]
    fn media_item_to_song_preserves_thumbnail_for_tracks() {
        use crate::domain::{
            MediaItem,
            media_item::{BackendExtension, Track, YouTubeData},
        };

        // Arrange: Create MediaItem::Track with thumbnail
        let item = MediaItem::Track(Track {
            id: "video123".to_string(),
            title: "Search Result Song".to_string(),
            artist: Some("Artist Name".to_string()),
            album: None,
            duration: Some(std::time::Duration::from_secs(240)),
            thumbnail: Some("https://ytimg.com/vi/video123/thumb.jpg".to_string()),
            explicit: false,
            backend: BackendExtension::YouTube(YouTubeData {
                video_id: Some("video123".to_string()),
                ..Default::default()
            }),
        });

        // Act: Convert to Song
        let song = Song::from(item);

        assert_eq!(
            song.thumbnail_url(),
            Some("https://ytimg.com/vi/video123/thumb.jpg"),
            "BUG: MediaItem to Song should preserve thumbnail for Track variant"
        );
    }

    #[test]
    fn media_item_to_song_preserves_thumbnail_for_artists() {
        use crate::domain::{
            MediaItem,
            media_item::{Artist, BackendExtension},
        };

        // Arrange: Create MediaItem::Artist with thumbnail
        let item = MediaItem::Artist(Artist {
            id: "UC12345".to_string(),
            name: "Famous Artist".to_string(),
            subscribers: Some("10M subscribers".to_string()),
            thumbnail: Some("https://yt3.ggpht.com/artist.jpg".to_string()),
            description: None,
            backend: BackendExtension::None,
        });

        // Act: Convert to Song
        let song = Song::from(item);

        assert_eq!(
            song.thumbnail_url(),
            Some("https://yt3.ggpht.com/artist.jpg"),
            "BUG: MediaItem to Song should preserve thumbnail for Artist variant"
        );
    }

    #[test]
    fn media_item_to_song_preserves_thumbnail_for_albums() {
        use crate::domain::{
            MediaItem,
            media_item::{Album, BackendExtension},
        };

        // Arrange: Create MediaItem::Album with thumbnail
        let item = MediaItem::Album(Album {
            id: "MPREb_album123".to_string(),
            title: "Greatest Hits".to_string(),
            artist: Some("Some Artist".to_string()),
            year: Some(2023),
            track_count: Some(12),
            thumbnail: Some("https://lh3.googleusercontent.com/album.jpg".to_string()),
            explicit: false,
            backend: BackendExtension::None,
        });

        // Act: Convert to Song
        let song = Song::from(item);

        assert_eq!(
            song.thumbnail_url(),
            Some("https://lh3.googleusercontent.com/album.jpg"),
            "BUG: MediaItem to Song should preserve thumbnail for Album variant"
        );
    }

    // =========================================================================
    // IPC Serialization Round-Trip Tests
    // =========================================================================
    //
    // These tests verify that MediaItem survives JSON serialization
    // and deserialization (simulating the IPC path between daemon and TUI).

    #[test]
    fn media_item_survives_json_roundtrip_with_thumbnail() {
        use crate::{
            backends::youtube::protocol::ServerResponse,
            domain::{
                MediaItem,
                media_item::{BackendExtension, Track, YouTubeData},
            },
        };

        // Arrange: Create MediaItem with thumbnail
        let original = MediaItem::Track(Track {
            id: "xyz789".to_string(),
            title: "IPC Test Song".to_string(),
            artist: Some("IPC Artist".to_string()),
            album: Some("IPC Album".to_string()),
            duration: Some(std::time::Duration::from_secs(300)),
            thumbnail: Some("https://ipc.test/thumbnail.jpg".to_string()),
            explicit: false,
            backend: BackendExtension::YouTube(YouTubeData {
                video_id: Some("xyz789".to_string()),
                ..Default::default()
            }),
        });

        // Wrap in ServerResponse (as the daemon does)
        let response = ServerResponse::SearchResults(vec![original.clone()]);

        // Act: Serialize to JSON (daemon → IPC)
        let json = serde_json::to_string(&response).expect("Serialization should succeed");

        // Log the JSON for debugging
        println!("[DIAG-IPC] Serialized JSON: {}", json);

        // Act: Deserialize from JSON (IPC → TUI)
        let deserialized: ServerResponse =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        // Extract the items
        let items = match deserialized {
            ServerResponse::SearchResults(items) => items,
            _ => panic!("Expected SearchResults variant"),
        };

        assert_eq!(items.len(), 1, "Should have exactly one item");

        // Verify thumbnail survived the round-trip
        let item = &items[0];
        match item {
            MediaItem::Track(t) => {
                assert_eq!(
                    t.thumbnail,
                    Some("https://ipc.test/thumbnail.jpg".to_string()),
                    "BUG: Thumbnail should survive JSON serialization round-trip"
                );
            }
            _ => panic!("Expected Track variant"),
        }

        // Now test the full conversion pipeline after deserialization
        let song = Song::from(items.into_iter().next().unwrap());

        assert_eq!(
            song.thumbnail_url(),
            Some("https://ipc.test/thumbnail.jpg"),
            "BUG: Thumbnail should be preserved after IPC round-trip and conversion"
        );
    }

    #[test]
    fn media_item_artist_survives_json_roundtrip_with_thumbnail() {
        use crate::{
            backends::youtube::protocol::ServerResponse,
            domain::{
                MediaItem,
                media_item::{Artist, BackendExtension},
            },
        };

        // Arrange: Create Artist with thumbnail
        let original = MediaItem::Artist(Artist {
            id: "UCBR8-60-B28hp2BmDPdntcQ".to_string(),
            name: "YouTube Channel".to_string(),
            subscribers: Some("50M subscribers".to_string()),
            thumbnail: Some("https://yt3.ggpht.com/channel.jpg".to_string()),
            description: None,
            backend: BackendExtension::None,
        });

        let response = ServerResponse::SearchResults(vec![original]);
        let json = serde_json::to_string(&response).expect("Serialization should succeed");

        println!("[DIAG-IPC] Artist JSON: {}", json);

        let deserialized: ServerResponse =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        let items = match deserialized {
            ServerResponse::SearchResults(items) => items,
            _ => panic!("Expected SearchResults"),
        };

        match &items[0] {
            MediaItem::Artist(a) => {
                assert_eq!(
                    a.thumbnail,
                    Some("https://yt3.ggpht.com/channel.jpg".to_string()),
                    "BUG: Artist thumbnail should survive JSON round-trip"
                );
            }
            _ => panic!("Expected Artist variant"),
        }
    }
}
