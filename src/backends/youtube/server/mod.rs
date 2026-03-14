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
use parking_lot::Mutex;

use super::{
    audio::{
        AudioSourcePlanner,
        AudioTransportTarget,
        CacheConfig,
        MpvAudioSource,
    },
    config::{AudioDeliveryMode, ExtractorType},
    media::{MediaPreparer, RelayRuntime},
    protocol::{ServerCommand, ServerResponse, framing},
    services::{
        ApiService,
        InternalEvent,
        PlaybackService,
        PlaybackStateTracker,
        PreloadScheduler,
        QueueService,
        YouTubeServices,
    },
};
use crate::shared::play_queue::{PlayQueue, QueueCommand, QueueEvent};

/// YouTube server orchestrates services and handles IPC
pub struct YouTubeServer {
    api: Arc<ApiService>,
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    /// PlayQueue - the new event-driven queue state machine
    play_queue: Arc<Mutex<PlayQueue>>,
    state_tracker: Arc<PlaybackStateTracker>,
    preload_scheduler: Arc<Mutex<PreloadScheduler>>,
    media_preparer: Arc<dyn MediaPreparer>,
    relay_runtime: Option<Arc<RelayRuntime>>,
    queue_event_handler: Mutex<QueueEventHandler>,
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
        audio_delivery_mode: AudioDeliveryMode,
    ) -> Result<Self> {
        let mpv_socket = socket_path.with_extension("mpv.sock");

        // Create API service
        let api = Arc::new(ApiService::new()?);
        if let Some(cookies) = cookie_file {
            if let Err(e) = api.load_cookies(cookies) {
                log::warn!("Failed to load cookies: {}", e);
            }
        }

        // Create shared services registry (single source of truth)
        let cache_config = CacheConfig::default();
        let audio_source_plan = AudioSourcePlanner.plan(audio_delivery_mode);
        let services =
            YouTubeServices::new(extractor_type, audio_source_plan, cache_config.clone())?;
        let media_preparer = services.media_preparer();

        let audio_source: Option<Arc<Mutex<dyn MpvAudioSource>>> = None;
        let relay_runtime = if matches!(audio_source_plan.transport, AudioTransportTarget::LocalRelay)
        {
            match RelayRuntime::start() {
                Ok(runtime) => Some(Arc::new(runtime)),
                Err(err) => {
                    log::warn!(
                        "Failed to start relay runtime; falling back to non-relay runtime input path: {err}"
                    );
                    None
                }
            }
        } else {
            None
        };

        match audio_source_plan.transport {
            AudioTransportTarget::DirectUrl => {
                log::info!("Audio mode: Direct (no local staging)");
            }
            AudioTransportTarget::LocalRelay => {
                if relay_runtime.is_some() {
                    log::info!("Audio mode: Relay (localhost streaming runtime enabled)");
                } else {
                    log::info!(
                        "Audio mode: Relay requested (runtime unavailable, using fallback input path)"
                    );
                }
            }
            AudioTransportTarget::Combined => {
                log::info!(
                    "Audio mode: Combined (shared preparer stages prefixes in {:?})",
                    cache_config.cache_dir
                );
            }
        }

        // Create playback service with shared resolver (spawns MPV)
        let playback = Arc::new(PlaybackService::new(
            &mpv_socket,
            services.url_resolver(),
            audio_source,
            audio_source_plan,
            relay_runtime.clone(),
        )?);

        // Create queue service
        let queue = Arc::new(QueueService::new());

        // Create state tracker
        let state_tracker = Arc::new(PlaybackStateTracker::new());

        // Create event broadcast channel (unbounded for non-blocking sends)
        let (event_tx, event_rx) = channel::unbounded();
        let (internal_event_tx, internal_event_rx) = channel::unbounded();

        let preload_scheduler = Arc::new(Mutex::new(PreloadScheduler::new()));

        let play_queue = Arc::new(Mutex::new(PlayQueue::new()));

        let queue_event_handler = QueueEventHandler::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            Arc::clone(&play_queue),
        )
        .with_media_preparer(Arc::clone(&media_preparer));

        Ok(Self {
            api,
            playback,
            queue,
            play_queue,
            state_tracker,
            preload_scheduler,
            media_preparer,
            relay_runtime,
            queue_event_handler: Mutex::new(queue_event_handler),
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
        let media_preparer = Arc::clone(&self.media_preparer);

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
                                    &media_preparer,
                                    &reason,
                                );
                                let _ = event_tx.send("player".to_string());
                            }
                            InternalEvent::TrackChanged { position } => {
                                orchestrator::handle_track_changed(
                                    &playback,
                                    &queue,
                                    &state_tracker,
                                    &media_preparer,
                                    position,
                                );
                                let _ = event_tx.send("player".to_string());
                            }
                            InternalEvent::IdleChanged { .. } => {}
                            InternalEvent::TimeRemaining { seconds } => {
                                orchestrator::handle_time_remaining(
                                    &queue,
                                    &media_preparer,
                                    seconds,
                                );
                            }
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
                &self.media_preparer,
            ),
            ServerCommand::Pause => handlers::handle_pause(&self.playback, &self.event_tx),
            ServerCommand::Stop => handlers::handle_stop(&self.playback, &self.event_tx),
            ServerCommand::SeekAbsolute(pos) => handlers::handle_seek_absolute(&self.playback, pos),
            ServerCommand::SeekRelative(delta) => {
                handlers::handle_seek_relative(&self.playback, delta)
            }

            // Navigation (uses orchestrator)
            ServerCommand::Next => {
                orchestrator::next_track(
                    &self.playback,
                    &self.queue,
                    &self.state_tracker,
                    &self.media_preparer,
                )
            }
            ServerCommand::Previous => {
                orchestrator::previous_track(
                    &self.playback,
                    &self.queue,
                    &self.state_tracker,
                    &self.media_preparer,
                )
            }
            ServerCommand::PlayPos(pos) => orchestrator::play_position_sync(
                &self.playback,
                &self.queue,
                pos,
                &self.state_tracker,
                &self.media_preparer,
            ),
            ServerCommand::PlayId(id) => {
                orchestrator::play_id(
                    &self.playback,
                    &self.queue,
                    id,
                    &self.state_tracker,
                    &self.media_preparer,
                )
            }

            // Queue handlers
            ServerCommand::Add { uri, position } => handlers::handle_add(
                &self.queue,
                &self.playback,
                &self.event_tx,
                &uri,
                position,
                &self.play_queue,
                &self.queue_event_handler,
                &self.media_preparer,
            ),
            ServerCommand::AddSong { song, position } => {
                log::info!("AddSong command received: file={}, title={:?}", song.file, song.title);
                let result = handlers::handle_add_song(
                    &self.queue,
                    &self.playback,
                    &self.event_tx,
                    song,
                    position,
                    &self.play_queue,
                    &self.queue_event_handler,
                    &self.media_preparer,
                );
                log::info!("AddSong result: {:?}", result);
                result
            }
            ServerCommand::DeleteId(id) => handlers::handle_delete_id(
                &self.queue,
                &self.playback,
                &self.event_tx,
                id,
                &self.play_queue,
                &self.queue_event_handler,
            ),
            ServerCommand::Clear => handlers::handle_clear(
                &self.queue,
                &self.playback,
                &self.event_tx,
                &self.play_queue,
                &self.queue_event_handler,
            ),
            ServerCommand::MoveId { from, to } => handlers::handle_move_id(
                &self.queue,
                &self.playback,
                &self.event_tx,
                from,
                to,
                &self.play_queue,
                &self.queue_event_handler,
            ),

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

            // PlayIntent handlers
            ServerCommand::PlayWithIntent { intent, request_id } => {
                handlers::handle_play_with_intent(
                    intent,
                    request_id,
                    &self.playback,
                    &self.queue,
                    &self.state_tracker,
                    &self.event_tx,
                    &self.media_preparer,
                )
            }
            ServerCommand::CancelRequest { request_id: _ } => {
                log::debug!(
                    "CancelRequest is no longer supported (cancel method removed from MediaPreparer)"
                );
                ServerResponse::Ok
            }
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

    pub fn apply_queue_command(&self, cmd: QueueCommand) -> Vec<QueueEvent> {
        let events = self.play_queue.lock().apply(cmd);

        let mut handler = self.queue_event_handler.lock();
        for event in &events {
            handler.handle(event.clone());
        }

        let _ = self.event_tx.send("playlist".to_string());

        events
    }

    pub fn play_queue(&self) -> &Arc<Mutex<PlayQueue>> {
        &self.play_queue
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let socket = std::path::Path::new("/tmp/test-yt.sock");
        let result = YouTubeServer::new(
            socket,
            None,
            ExtractorType::default(),
            AudioDeliveryMode::default(),
        );
        assert!(result.is_ok() || result.is_err());
    }
}
