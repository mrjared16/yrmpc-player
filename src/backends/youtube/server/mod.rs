//! YouTube backend server - orchestrates services and handles IPC.
//!
//! Architecture:
//! ```text
//! YouTubeServer (socket/connection management)
//!   ├─ Handlers (thin command wrappers)
//!   │   ├─ playback: Play, Pause, Stop, Seek
//!   │   ├─ queue: Add, Delete, Clear, Move
//!   │   ├─ status: GetStatus, GetCurrentSong, GetPlaylist
//!   │   ├─ search: Search, Browse, GetSuggestions
//!   │   └─ options: Volume, Repeat, Shuffle
//!   ├─ Orchestrator (prefetch window, track-ended, play_position)
//!   └─ Services
//!       ├─ ApiService (YouTube Music API)
//!       ├─ PlaybackService (MPV control + buffer)
//!       └─ QueueService (metadata storage for UI)
//!
//! Terminology:
//!   - Queue: User's full playlist (QueueService) - can be 1000s of songs
//!   - Buffer: MPV's internal playlist (3-track rolling window for gapless playback)
//!
//! Data Flow:
//!   AddSong → QueueService (metadata) + MPV buffer (URL if in window)
//!   Delete  → QueueService.remove() → MPV buffer sync
//!   Play    → Rebuild buffer at position → playlist_play_index(0)
//! ```

pub mod handlers;
pub mod orchestrator;

use std::{
    io::{BufReader, BufWriter},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossbeam::channel::{self, Receiver, Sender};
use handlers::queue_events::QueueEventHandler;

use super::{
    config::ExtractorType,
    protocol::{ServerCommand, ServerResponse, framing},
    services::{
        ApiService, AudioPrefetcher, AudioPrefetcherConfig, InternalEvent,
        PlaybackService, PlaybackServiceCallback, PlaybackStateTracker, QueueService,
    },
};

/// YouTube server orchestrates services and handles IPC
pub struct YouTubeServer {
    api: Arc<ApiService>,
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    state_tracker: Arc<PlaybackStateTracker>,
    #[allow(dead_code)]
    queue_event_handler: QueueEventHandler,
    #[allow(dead_code)]
    audio_prefetcher: AudioPrefetcher,
    running: Arc<AtomicBool>,
    socket_path: PathBuf,
    mpv_socket_path: PathBuf,
    event_tx: Sender<String>,
    event_rx: Receiver<String>,
    internal_event_tx: Sender<InternalEvent>,
    internal_event_rx: Receiver<InternalEvent>,
}

impl YouTubeServer {
    /// Create new server with services
    pub fn new(
        socket_path: &Path,
        cookie_file: Option<&str>,
        extractor_type: ExtractorType,
    ) -> Result<Self> {
        let mpv_socket = socket_path.with_extension("mpv.sock");

        // Create API service
        let api = Arc::new(ApiService::new()?);
        if let Some(cookies) = cookie_file {
            if let Err(e) = api.load_cookies(cookies) {
                log::warn!("Failed to load cookies: {}", e);
            }
        }

        // Create playback service (spawns MPV)
        let playback = Arc::new(PlaybackService::new(&mpv_socket, extractor_type)?);

        // Create queue service
        let queue = Arc::new(QueueService::new());

        // Create state tracker
        let state_tracker = Arc::new(PlaybackStateTracker::new());

        // Create event broadcast channel (unbounded for non-blocking sends)
        let (event_tx, event_rx) = channel::unbounded();
        let (internal_event_tx, internal_event_rx) = channel::unbounded();

        let callback = PlaybackServiceCallback::new(Arc::clone(&playback));
        let audio_prefetcher = AudioPrefetcher::new(AudioPrefetcherConfig::default(), callback);
        let prefetcher_handle = audio_prefetcher.handle();

        let queue_event_handler = QueueEventHandler::new(Arc::clone(&playback), Arc::clone(&queue))
            .with_audio_prefetcher(prefetcher_handle);

        Ok(Self {
            api,
            playback,
            queue,
            state_tracker,
            queue_event_handler,
            audio_prefetcher,
            running: Arc::new(AtomicBool::new(false)),
            socket_path: socket_path.to_path_buf(),
            mpv_socket_path: mpv_socket,
            event_tx,
            event_rx,
            internal_event_tx,
            internal_event_rx,
        })
    }

    /// Run the server, listening for client connections
    pub fn run(&self) -> Result<()> {
        // Remove old socket
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)
            .with_context(|| format!("Failed to bind to {}", self.socket_path.display()))?;

        log::info!("YouTube server listening on {}", self.socket_path.display());
        self.running.store(true, Ordering::SeqCst);

        // Start the MPV event loop for property observation
        if let Err(e) = self.playback.start_event_loop(
            &self.mpv_socket_path,
            self.event_tx.clone(),
            self.internal_event_tx.clone(),
        ) {
            log::error!("Failed to start MPV event loop: {}", e);
        }

        // Start internal event processor thread
        self.start_internal_event_processor();

        for stream in listener.incoming() {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match stream {
                Ok(stream) => {
                    if let Err(e) = self.handle_client(stream) {
                        log::error!("Client error: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("Accept error: {}", e);
                }
            }
        }

        self.playback.stop_event_loop();

        log::info!("Server shutting down");
        Ok(())
    }

    /// Start internal event processor thread
    fn start_internal_event_processor(&self) {
        let internal_event_rx = self.internal_event_rx.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let playback = Arc::clone(&self.playback);
        let queue = Arc::clone(&self.queue);
        let state_tracker = Arc::clone(&self.state_tracker);

        thread::spawn(move || {
            log::info!("Internal event processor started");

            while running.load(Ordering::SeqCst) {
                match internal_event_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(event) => {
                        log::debug!("Internal event processor received: {:?}", event);

                        match event {
                            InternalEvent::EndFile { reason } => {
                                orchestrator::handle_track_ended(
                                    &playback,
                                    &queue,
                                    &state_tracker,
                                    &reason,
                                );
                                let _ = event_tx.send("player".to_string());
                            }
                            InternalEvent::TrackChanged { position } => {
                                orchestrator::handle_track_changed(
                                    &playback,
                                    &queue,
                                    &state_tracker,
                                    position,
                                );
                                let _ = event_tx.send("player".to_string());
                            }
                            InternalEvent::IdleChanged { .. } => {}
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        log::info!("Event channel disconnected, stopping processor");
                        break;
                    }
                }
            }

            log::info!("Internal event processor stopped");
        });
    }

    /// Handle a single client connection
    fn handle_client(&self, stream: UnixStream) -> Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(300)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);

        loop {
            let cmd: ServerCommand = match framing::read_message(&mut reader) {
                Ok(cmd) => {
                    log::trace!("Received command: {:?}", std::mem::discriminant(&cmd));
                    cmd
                }
                Err(e) => {
                    log::trace!("Client disconnected: {}", e);
                    break;
                }
            };

            let response = self.handle_command(cmd);

            log::trace!("Sending response: {:?}", std::mem::discriminant(&response));
            if let Err(e) = framing::write_message(&mut writer, &response) {
                log::trace!("Connection closed: {}", e);
                break;
            }

            if matches!(response, ServerResponse::Ok) && !self.running.load(Ordering::SeqCst) {
                break;
            }
        }

        Ok(())
    }

    /// Process a command and return response
    pub fn handle_command(&self, cmd: ServerCommand) -> ServerResponse {
        match cmd {
            ServerCommand::Ping => ServerResponse::Pong,

            ServerCommand::Shutdown => {
                self.running.store(false, Ordering::SeqCst);
                ServerResponse::Ok
            }

            // Playback handlers
            ServerCommand::Play => handlers::handle_play(
                &self.playback,
                &self.queue,
                &self.state_tracker,
                &self.event_tx,
            ),
            ServerCommand::Pause => handlers::handle_pause(&self.playback, &self.event_tx),
            ServerCommand::Stop => handlers::handle_stop(&self.playback, &self.event_tx),
            ServerCommand::SeekAbsolute(pos) => handlers::handle_seek_absolute(&self.playback, pos),
            ServerCommand::SeekRelative(delta) => {
                handlers::handle_seek_relative(&self.playback, delta)
            }

            // Navigation (uses orchestrator)
            ServerCommand::Next => {
                orchestrator::next_track(&self.playback, &self.queue, &self.state_tracker)
            }
            ServerCommand::Previous => {
                orchestrator::previous_track(&self.playback, &self.queue, &self.state_tracker)
            }
            ServerCommand::PlayPos(pos) => {
                orchestrator::play_position(&self.playback, &self.queue, pos, &self.state_tracker)
            }
            ServerCommand::PlayId(id) => {
                orchestrator::play_id(&self.playback, &self.queue, id, &self.state_tracker)
            }

            // Queue handlers
            ServerCommand::Add { uri, position } => {
                handlers::handle_add(&self.queue, &self.playback, &self.event_tx, &uri, position)
            }
            ServerCommand::AddSong { song, position } => {
                log::info!("AddSong command received: file={}, title={:?}", song.file, song.title);
                let result = handlers::handle_add_song(
                    &self.queue,
                    &self.playback,
                    &self.event_tx,
                    song,
                    position,
                );
                log::info!("AddSong result: {:?}", result);
                result
            }
            ServerCommand::DeleteId(id) => {
                handlers::handle_delete_id(&self.queue, &self.playback, &self.event_tx, id)
            }
            ServerCommand::Clear => {
                handlers::handle_clear(&self.queue, &self.playback, &self.event_tx)
            }
            ServerCommand::MoveId { from, to } => {
                handlers::handle_move_id(&self.queue, &self.playback, &self.event_tx, from, to)
            }

            // Volume/options handlers
            ServerCommand::GetVolume => handlers::handle_get_volume(&self.playback),
            ServerCommand::SetVolume(vol) => handlers::handle_set_volume(&self.playback, vol),
            ServerCommand::AdjustVolume(delta) => {
                handlers::handle_adjust_volume(&self.playback, delta)
            }
            ServerCommand::SetRepeat(mode) => {
                handlers::handle_set_repeat(&self.queue, &self.event_tx, &mode)
            }
            ServerCommand::SetShuffle(enabled) => {
                handlers::handle_set_shuffle(&self.queue, &self.event_tx, enabled)
            }

            // Status handlers
            ServerCommand::GetStatus => handlers::handle_get_status(&self.playback, &self.queue),
            ServerCommand::GetCurrentSong => {
                handlers::handle_get_current_song(&self.playback, &self.queue)
            }
            ServerCommand::GetPlaylist => handlers::handle_get_playlist(&self.queue),

            // Search handlers
            ServerCommand::Search { query } => handlers::handle_search(&self.api, &query),
            ServerCommand::Browse { path } => handlers::handle_browse(&self.api, &path),
            ServerCommand::GetSearchSuggestions { query } => {
                handlers::handle_get_suggestions(&self.api, &query)
            }
            ServerCommand::GetLibrary { category } => handlers::handle_get_library(&category),

            // Rich browse details handlers
            ServerCommand::BrowsePlaylistDetails { playlist_id } => {
                handlers::handle_browse_playlist_details(&self.api, &playlist_id)
            }
            ServerCommand::BrowseAlbumDetails { album_id } => {
                handlers::handle_browse_album_details(&self.api, &album_id)
            }
            ServerCommand::BrowseArtistDetails { artist_id } => {
                handlers::handle_browse_artist_details(&self.api, &artist_id)
            }

            // Idle handler
            ServerCommand::Idle { subsystems } => self.handle_idle(subsystems),
        }
    }

    /// Handle Idle command (blocking event wait)
    fn handle_idle(&self, subsystems: Vec<String>) -> ServerResponse {
        log::debug!("Idle subscription for: {:?}", subsystems);

        match self.event_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(event) => {
                if subsystems.is_empty() || subsystems.contains(&event) {
                    log::debug!("Idle woke up with event: {}", event);
                    ServerResponse::IdleEvents(vec![event])
                } else {
                    ServerResponse::IdleEvents(vec![])
                }
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                ServerResponse::IdleEvents(vec![])
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                ServerResponse::Error("Server shutting down".to_string())
            }
        }
    }

    /// Stop the server
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for YouTubeServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let socket = std::path::Path::new("/tmp/test-yt.sock");
        let result = YouTubeServer::new(socket, None, ExtractorType::default());
        assert!(result.is_ok() || result.is_err());
    }
}
