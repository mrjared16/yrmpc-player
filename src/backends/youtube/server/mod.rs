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
pub mod playback_coordinator;
pub mod playback_horizon;
pub(crate) mod playback_prepare;
pub mod prefetch_manager;
pub mod queue_coordinator;
pub mod queue_view;
#[cfg(test)]
pub(crate) mod test_support;
pub mod track_ended;

use std::{
    io::{BufReader, BufWriter},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossbeam::channel::{self, Receiver, Sender};
use handlers::queue_events::QueueEventHandler;
use parking_lot::Mutex;

use super::{
    audio::{AudioDeliveryPlanner, AudioTransportTarget, CacheConfig, MpvAudioSource},
    config::{AudioDeliveryMode, BackgroundExtractMode, ExtractorType, YtDlpExtractorConfig},
    media::{MediaPreparer, RelayRuntime},
    protocol::{CLIENT_SUPERSEDED_ERROR, ServerCommand, ServerResponse, framing},
    services::{
        ApiService, InternalEvent, PlaybackService, PlaybackStateTracker, PreloadScheduler,
        QueueService, YouTubeServices,
    },
};
use crate::shared::play_queue::PlayQueue;
use orchestrator::Orchestrator;
use queue_coordinator::QueueCoordinator;

fn normalize_subsystem_name(name: &str) -> &str {
    match name {
        // Historical alias used by some handlers
        "queue" => "playlist",
        other => other,
    }
}

#[derive(Clone)]
struct ActiveIdleOwner {
    session_id: u64,
    tx: Sender<String>,
}

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
    orchestrator: Arc<Orchestrator>,
    queue_coordinator: Arc<QueueCoordinator>,
    running: Arc<AtomicBool>,
    next_client_session_id: AtomicU64,
    latest_session_id: AtomicU64,
    socket_path: PathBuf,
    mpv_socket_path: PathBuf,
    event_tx: Sender<String>,
    event_rx: Receiver<String>,
    idle_owner: Arc<Mutex<Option<ActiveIdleOwner>>>,
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
        background_extract_mode: BackgroundExtractMode,
        future_track_count: usize,
        ytdlp_config: YtDlpExtractorConfig,
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
        let audio_source_plan = AudioDeliveryPlanner.plan(audio_delivery_mode);
        let services = YouTubeServices::new(
            extractor_type,
            ytdlp_config,
            audio_source_plan,
            cache_config.clone(),
        )?;
        let media_preparer = services.media_preparer();

        let audio_source: Option<Arc<Mutex<dyn MpvAudioSource>>> = None;
        let relay_runtime = if matches!(
            audio_source_plan.transport,
            AudioTransportTarget::LocalRelay
        ) {
            match RelayRuntime::start_with_cache(
                services.audio_cache(),
                Some(services.url_resolver()),
            ) {
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
            AudioTransportTarget::PreparedInput => {
                log::info!(
                    "Audio mode: Staged (shared preparer stages prefixes in {:?})",
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

        let orchestrator = Arc::new(Orchestrator::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            Arc::clone(&state_tracker),
            Arc::clone(&media_preparer),
            background_extract_mode,
            future_track_count,
        ));

        let queue_event_handler = QueueEventHandler::new(
            Arc::clone(&playback),
            Arc::clone(&queue),
            Arc::clone(&play_queue),
        )
        .with_media_preparer(Arc::clone(&media_preparer))
        .with_playback_coordinator(Arc::clone(orchestrator.coordinator()))
        .with_plan_changed({
            let orchestrator = Arc::clone(&orchestrator);
            Arc::new(move || orchestrator.publish_prefix_plan_changed())
        });

        let queue_coordinator = Arc::new(QueueCoordinator::new(
            Arc::clone(&queue),
            Arc::clone(&playback),
            Arc::clone(&play_queue),
            queue_event_handler,
            Arc::clone(&orchestrator),
            event_tx.clone(),
        ));

        Ok(Self {
            api,
            playback,
            queue,
            play_queue,
            state_tracker,
            preload_scheduler,
            media_preparer,
            relay_runtime,
            orchestrator,
            queue_coordinator,
            running: Arc::new(AtomicBool::new(false)),
            next_client_session_id: AtomicU64::new(1),
            latest_session_id: AtomicU64::new(0),
            socket_path: socket_path.to_path_buf(),
            mpv_socket_path: mpv_socket,
            event_tx,
            event_rx,
            idle_owner: Arc::new(Mutex::new(None)),
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
        self.start_event_fanout_processor();

        thread::scope(|scope| {
            for stream in listener.incoming() {
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }

                match stream {
                    Ok(stream) => {
                        let session_id =
                            self.next_client_session_id.fetch_add(1, Ordering::Relaxed);
                        self.latest_session_id.store(session_id, Ordering::SeqCst);
                        let server = self;
                        scope.spawn(move || {
                            if let Err(e) = server.handle_client_session(session_id, stream) {
                                log::error!("Client session {} error: {}", session_id, e);
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("Accept error: {}", e);
                    }
                }
            }
        });

        self.playback.stop_event_loop();

        log::info!("Server shutting down");
        Ok(())
    }

    /// Start internal event processor thread
    fn start_internal_event_processor(&self) {
        let internal_event_rx = self.internal_event_rx.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let orchestrator = Arc::clone(&self.orchestrator);

        thread::spawn(move || {
            log::info!("Internal event processor started");

            while running.load(Ordering::SeqCst) {
                match internal_event_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(event) => match event {
                        InternalEvent::EndFile { reason } => {
                            orchestrator.handle_track_ended(&reason);
                            let _ = event_tx.send("player".to_string());
                        }
                        InternalEvent::TrackChanged { position } => {
                            orchestrator.handle_track_changed(position);
                            let _ = event_tx.send("player".to_string());
                        }
                        InternalEvent::IdleChanged { .. } => {}
                        InternalEvent::PlaybackStarted => {
                            orchestrator.handle_playback_started();
                        }
                    },
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

    /// Start processor to publish backend events only to the latest idle owner.
    fn start_event_fanout_processor(&self) {
        let event_rx = self.event_rx.clone();
        let running = Arc::clone(&self.running);
        let idle_owner = Arc::clone(&self.idle_owner);

        thread::spawn(move || {
            log::info!("Idle owner event processor started");

            while running.load(Ordering::SeqCst) {
                match event_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(event) => {
                        let event = normalize_subsystem_name(&event).to_string();
                        let active_owner = idle_owner.lock().clone();

                        if let Some(active_owner) = active_owner {
                            if active_owner.tx.send(event.clone()).is_err() {
                                log::trace!(session_id = active_owner.session_id; "Dropping stale idle owner receiver");

                                let mut owner_guard = idle_owner.lock();
                                if owner_guard.as_ref().is_some_and(|owner| {
                                    owner.session_id == active_owner.session_id
                                }) {
                                    *owner_guard = None;
                                }
                            } else {
                                log::trace!(session_id = active_owner.session_id, event = event.as_str(); "Published event to idle owner");
                            }
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        log::info!("Event queue disconnected, stopping idle owner processor");
                        break;
                    }
                }
            }

            log::info!("Idle owner event processor stopped");
        });
    }

    /// Handle one connected client session lifecycle.
    fn handle_client_session(&self, session_id: u64, stream: UnixStream) -> Result<()> {
        log::debug!("Client session {} connected", session_id);
        let (session_event_tx, session_event_rx) = channel::unbounded();
        let mut pending_initial_snapshot = true;

        stream.set_read_timeout(Some(Duration::from_secs(300)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);

        loop {
            let cmd: ServerCommand = match framing::read_message(&mut reader) {
                Ok(cmd) => {
                    log::trace!(
                        "Session {} received command: {:?}",
                        session_id,
                        std::mem::discriminant(&cmd)
                    );
                    cmd
                }
                Err(e) => {
                    log::trace!("Session {} disconnected: {}", session_id, e);
                    break;
                }
            };

            let response = if self.is_superseded_session(session_id) {
                ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string())
            } else {
                match cmd {
                    ServerCommand::Idle { subsystems } => {
                        if !self.ensure_idle_ownership(session_id, session_event_tx.clone()) {
                            ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string())
                        } else {
                            if pending_initial_snapshot {
                                pending_initial_snapshot = false;
                                let snapshot_events =
                                    self.initial_snapshot_idle_events(&subsystems);
                                if !snapshot_events.is_empty() {
                                    ServerResponse::IdleEvents(snapshot_events)
                                } else {
                                    self.handle_idle_for_session(
                                        session_id,
                                        &session_event_rx,
                                        subsystems,
                                    )
                                }
                            } else {
                                self.handle_idle_for_session(
                                    session_id,
                                    &session_event_rx,
                                    subsystems,
                                )
                            }
                        }
                    }
                    other => self.handle_non_idle_command(other),
                }
            };

            log::trace!(
                "Session {} sending response: {:?}",
                session_id,
                std::mem::discriminant(&response)
            );
            if let Err(e) = framing::write_message(&mut writer, &response) {
                log::trace!("Session {} connection closed: {}", session_id, e);
                break;
            }

            if matches!(&response, ServerResponse::Error(err) if err == CLIENT_SUPERSEDED_ERROR) {
                break;
            }

            if matches!(response, ServerResponse::Ok) && !self.running.load(Ordering::SeqCst) {
                break;
            }
        }

        self.release_idle_ownership(session_id);

        log::debug!("Client session {} closed", session_id);

        Ok(())
    }

    /// Process a non-idle command and return response.
    pub fn handle_non_idle_command(&self, cmd: ServerCommand) -> ServerResponse {
        match cmd {
            ServerCommand::Ping => ServerResponse::Pong,

            ServerCommand::Shutdown => {
                self.running.store(false, Ordering::SeqCst);
                ServerResponse::Ok
            }

            // Playback handlers
            ServerCommand::Play => handlers::handle_play(&self.orchestrator, &self.event_tx),
            ServerCommand::Pause => handlers::handle_pause(&self.playback, &self.event_tx),
            ServerCommand::Stop => handlers::handle_stop(&self.playback, &self.event_tx),
            ServerCommand::SeekAbsolute(pos) => handlers::handle_seek_absolute(&self.playback, pos),
            ServerCommand::SeekRelative(delta) => {
                handlers::handle_seek_relative(&self.playback, delta)
            }

            // Navigation (uses orchestrator)
            ServerCommand::Next => self.orchestrator.next_track(),
            ServerCommand::Previous => self.orchestrator.previous_track(),
            ServerCommand::PlayPos(pos) => self.orchestrator.play_position_sync(pos),
            ServerCommand::PlayId(id) => self.orchestrator.play_id(id),

            // Queue handlers
            ServerCommand::Add { uri, position } => {
                log::warn!("ServerCommand::Add is deprecated; prefer PlayWithIntent::Append");
                handlers::handle_add(&self.queue_coordinator, &uri, position)
            }
            ServerCommand::AddSong { song, position } => {
                log::warn!("ServerCommand::AddSong is deprecated; prefer PlayWithIntent::Append");
                log::info!("AddSong command received: file={}, title={:?}", song.file, song.title);
                let result = handlers::handle_add_song(&self.queue_coordinator, song, position);
                log::info!("AddSong result: {:?}", result);
                result
            }
            ServerCommand::DeleteId(id) => handlers::handle_delete_id(&self.queue_coordinator, id),
            ServerCommand::Clear => handlers::handle_clear(&self.queue_coordinator),
            ServerCommand::MoveId { from, to } => {
                handlers::handle_move_id(&self.queue_coordinator, from, to)
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
            ServerCommand::GetStatus => {
                handlers::handle_get_status(&self.playback, self.queue.as_ref())
            }
            ServerCommand::GetCurrentSong => {
                handlers::handle_get_current_song(&self.playback, self.queue.as_ref())
            }
            ServerCommand::GetPlaylist => handlers::handle_get_playlist(self.queue.as_ref()),

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

            // Idle is handled in handle_client_session with session-local subscriptions
            ServerCommand::Idle { .. } => {
                unreachable!("Idle command should be handled in handle_client_session")
            }

            // PlayIntent handlers
            ServerCommand::PlayWithIntent { intent, request_id } => {
                handlers::handle_play_with_intent(
                    intent,
                    request_id,
                    &self.orchestrator,
                    &self.queue_coordinator,
                    &self.event_tx,
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

    /// Handle Idle command for a specific session subscription.
    fn handle_idle_for_session(
        &self,
        session_id: u64,
        session_event_rx: &Receiver<String>,
        subsystems: Vec<String>,
    ) -> ServerResponse {
        let normalized_subsystems: Vec<String> =
            subsystems.into_iter().map(|s| normalize_subsystem_name(&s).to_string()).collect();
        let deadline = Instant::now() + Duration::from_millis(100);

        while Instant::now() < deadline {
            if self.is_superseded_session(session_id) || !self.is_idle_owner(session_id) {
                return ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string());
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            match session_event_rx.recv_timeout(remaining) {
                Ok(event) => {
                    if self.is_superseded_session(session_id) || !self.is_idle_owner(session_id) {
                        return ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string());
                    }

                    if event == CLIENT_SUPERSEDED_ERROR {
                        return ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string());
                    }

                    let event = normalize_subsystem_name(&event).to_string();
                    if normalized_subsystems.is_empty() || normalized_subsystems.contains(&event) {
                        log::debug!("Session {} idle woke up with event: {}", session_id, event);
                        return ServerResponse::IdleEvents(vec![event]);
                    }
                    // Ignore non-subscribed subsystem events and keep waiting.
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    if self.is_superseded_session(session_id) {
                        return ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string());
                    }
                    break;
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    return ServerResponse::Error("Server shutting down".to_string());
                }
            }
        }

        if self.is_superseded_session(session_id) {
            return ServerResponse::Error(CLIENT_SUPERSEDED_ERROR.to_string());
        }

        ServerResponse::IdleEvents(vec![])
    }

    fn ensure_idle_ownership(&self, session_id: u64, session_event_tx: Sender<String>) -> bool {
        let mut idle_owner = self.idle_owner.lock();

        match idle_owner.as_ref() {
            None => {
                *idle_owner = Some(ActiveIdleOwner { session_id, tx: session_event_tx });
                log::debug!(session_id; "Claimed idle ownership (first owner)");
                true
            }
            Some(owner) if owner.session_id == session_id => {
                *idle_owner = Some(ActiveIdleOwner { session_id, tx: session_event_tx });
                true
            }
            Some(owner) if session_id > owner.session_id => {
                let previous_owner_tx = owner.tx.clone();
                let previous_owner = owner.session_id;
                *idle_owner = Some(ActiveIdleOwner { session_id, tx: session_event_tx });
                log::debug!(session_id, previous_owner; "Claimed idle ownership from older session");

                let _ = previous_owner_tx.send(CLIENT_SUPERSEDED_ERROR.to_string());

                true
            }
            Some(owner) => {
                log::trace!(session_id, owner = owner.session_id; "Session superseded by newer idle owner");
                false
            }
        }
    }

    fn release_idle_ownership(&self, session_id: u64) {
        let mut idle_owner = self.idle_owner.lock();
        if idle_owner.as_ref().is_some_and(|owner| owner.session_id == session_id) {
            *idle_owner = None;
            log::debug!(session_id; "Released idle ownership");
        }
    }

    fn is_idle_owner(&self, session_id: u64) -> bool {
        self.idle_owner.lock().as_ref().is_some_and(|owner| owner.session_id == session_id)
    }

    fn is_superseded_session(&self, session_id: u64) -> bool {
        session_id < self.latest_session_id.load(Ordering::SeqCst)
    }

    fn initial_snapshot_idle_events(&self, subsystems: &[String]) -> Vec<String> {
        const SNAPSHOT_EVENTS: [&str; 3] = ["player", "playlist", "options"];

        let normalized_subsystems: Vec<String> =
            subsystems.iter().map(|s| normalize_subsystem_name(s).to_string()).collect();

        if normalized_subsystems.is_empty() {
            return SNAPSHOT_EVENTS.into_iter().map(str::to_string).collect();
        }

        SNAPSHOT_EVENTS
            .into_iter()
            .filter(|event| normalized_subsystems.iter().any(|s| s == event))
            .map(str::to_string)
            .collect()
    }

    /// Stop the server
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
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
    use anyhow::{Context, Result};
    use std::{
        path::PathBuf,
        sync::Arc,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct RunningServer {
        socket_path: PathBuf,
        server: Arc<YouTubeServer>,
        handle: Option<thread::JoinHandle<Result<()>>>,
    }

    impl RunningServer {
        fn start() -> Result<Self> {
            let socket_path = PathBuf::from(format!(
                "/tmp/test-yt-{}-{}.sock",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));

            let server = Arc::new(YouTubeServer::new(
                &socket_path,
                None,
                ExtractorType::default(),
                AudioDeliveryMode::default(),
                BackgroundExtractMode::default(),
                2,
                YtDlpExtractorConfig::default(),
            )?);

            let thread_server = Arc::clone(&server);
            let handle = thread::spawn(move || thread_server.run());

            wait_for_socket(&socket_path, Duration::from_secs(2))?;

            Ok(Self { socket_path, server, handle: Some(handle) })
        }

        fn connect_client(&self, timeout: Duration) -> Result<UnixStream> {
            let stream = UnixStream::connect(&self.socket_path)
                .with_context(|| format!("failed to connect to {}", self.socket_path.display()))?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            Ok(stream)
        }
    }

    impl Drop for RunningServer {
        fn drop(&mut self) {
            self.server.stop();

            // Wake accept loop so it can observe running=false and exit.
            let _ = UnixStream::connect(&self.socket_path);

            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }

            // Allow detached internal worker loops to observe running=false and exit.
            thread::sleep(Duration::from_millis(600));

            let _ = std::fs::remove_file(&self.socket_path);
        }
    }

    fn wait_for_socket(path: &Path, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }

        anyhow::bail!("socket did not appear in time: {}", path.display())
    }

    fn send_command(stream: &UnixStream, cmd: &ServerCommand) -> Result<ServerResponse> {
        let mut writer = BufWriter::new(stream.try_clone()?);
        framing::write_message(&mut writer, cmd)?;

        let mut reader = BufReader::new(stream.try_clone()?);
        let response: ServerResponse = framing::read_message(&mut reader)?;
        Ok(response)
    }

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
            BackgroundExtractMode::default(),
            2,
            YtDlpExtractorConfig::default(),
        );
        assert!(result.is_ok() || result.is_err());
    }

    /// Regression test for yrmpc-jwq.2:
    /// A connected idle/quiet client must not prevent another client from
    /// sending a basic request (Ping) and receiving a response.
    #[test]
    fn test_second_client_ping_while_first_client_connected() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let server = RunningServer::start()?;

        // First client connects and stays quiet.
        let _first = server.connect_client(Duration::from_millis(250))?;

        // Give server a moment to accept first client.
        thread::sleep(Duration::from_millis(50));

        // Second client should still be able to ping immediately.
        let second = server.connect_client(Duration::from_millis(250))?;
        let response = send_command(&second, &ServerCommand::Ping)?;

        match response {
            ServerResponse::Pong => Ok(()),
            other => anyhow::bail!("expected Pong, got {:?}", other),
        }
    }

    #[test]
    fn test_newest_idle_client_supersedes_older_client() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let server = RunningServer::start()?;

        let client1 = server.connect_client(Duration::from_secs(2))?;
        let first_response = send_command(
            &client1,
            &ServerCommand::Idle { subsystems: vec!["options".to_string()] },
        )?;
        assert!(matches!(first_response, ServerResponse::IdleEvents(_)));

        let client2 = server.connect_client(Duration::from_secs(2))?;
        let second_response = send_command(
            &client2,
            &ServerCommand::Idle { subsystems: vec!["options".to_string()] },
        )?;
        assert!(matches!(second_response, ServerResponse::IdleEvents(_)));

        let old_command_response = send_command(&client1, &ServerCommand::SetShuffle(false));
        match old_command_response {
            Ok(ServerResponse::Error(err)) if err == CLIENT_SUPERSEDED_ERROR => {}
            Err(err)
                if err.to_string().contains("Connection reset by peer")
                    || err.to_string().contains("Broken pipe") => {}
            other => {
                anyhow::bail!("expected old non-idle command to be rejected, got {:?}", other)
            }
        }

        let response = send_command(&client2, &ServerCommand::SetShuffle(true))?;
        assert!(matches!(response, ServerResponse::Ok));

        Ok(())
    }

    #[test]
    fn test_superseded_client_stays_blocked_after_newer_disconnects() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let server = RunningServer::start()?;

        let client1 = server.connect_client(Duration::from_secs(2))?;
        let _ = send_command(
            &client1,
            &ServerCommand::Idle { subsystems: vec!["player".to_string()] },
        )?;

        let client2 = server.connect_client(Duration::from_secs(2))?;
        let _ = send_command(
            &client2,
            &ServerCommand::Idle { subsystems: vec!["player".to_string()] },
        )?;

        drop(client2);
        thread::sleep(Duration::from_millis(20));

        let old_command_response = send_command(&client1, &ServerCommand::SetShuffle(false));
        match old_command_response {
            Ok(ServerResponse::Error(err)) if err == CLIENT_SUPERSEDED_ERROR => {}
            Err(err)
                if err.to_string().contains("Connection reset by peer")
                    || err.to_string().contains("Broken pipe") => {}
            other => anyhow::bail!(
                "expected superseded client to remain blocked after newer disconnect, got {:?}",
                other
            ),
        }

        Ok(())
    }

    #[test]
    fn test_old_idle_waiter_is_superseded_when_new_client_connects() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let server = RunningServer::start()?;

        let client1 = server.connect_client(Duration::from_secs(2))?;
        let _ = send_command(
            &client1,
            &ServerCommand::Idle { subsystems: vec!["player".to_string()] },
        )?;

        let idle_wait = thread::spawn(move || {
            send_command(&client1, &ServerCommand::Idle { subsystems: vec!["player".to_string()] })
        });

        thread::sleep(Duration::from_millis(20));

        let client2 = server.connect_client(Duration::from_secs(2))?;
        let ping = send_command(&client2, &ServerCommand::Ping)?;
        assert!(matches!(ping, ServerResponse::Pong));

        let trigger = send_command(&client2, &ServerCommand::SetShuffle(true))?;
        assert!(matches!(trigger, ServerResponse::Ok));

        let idle_wait_response = idle_wait.join().expect("idle waiter thread panicked")?;
        match idle_wait_response {
            ServerResponse::Error(err) if err == CLIENT_SUPERSEDED_ERROR => {}
            other => {
                anyhow::bail!("expected superseded error for old idle waiter, got {:?}", other)
            }
        }

        Ok(())
    }

    #[test]
    fn test_first_idle_returns_snapshot_events() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let server = RunningServer::start()?;
        let client = server.connect_client(Duration::from_secs(2))?;

        let response = send_command(
            &client,
            &ServerCommand::Idle {
                subsystems: vec![
                    "player".to_string(),
                    "playlist".to_string(),
                    "options".to_string(),
                ],
            },
        )?;

        match response {
            ServerResponse::IdleEvents(events) => {
                for expected in ["player", "playlist", "options"] {
                    if !events.iter().any(|ev| ev == expected) {
                        anyhow::bail!(
                            "expected snapshot idle events to include '{}', got {:?}",
                            expected,
                            events
                        );
                    }
                }
                Ok(())
            }
            other => anyhow::bail!("expected IdleEvents, got {:?}", other),
        }
    }
}
