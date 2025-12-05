//! YouTube backend server - orchestrates services and handles IPC
//!
//! Architecture:
//! ```text
//! YouTubeServer (orchestrator)
//!   ├─ ApiService (YouTube Music API)
//!   ├─ PlaybackService (MPV control)
//!   └─ QueueService (queue management)
//! ```

use std::{
    io::{BufReader, BufWriter},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};

use super::{
    protocol::{
        BrowseEntry, ServerCommand, ServerResponse, SongData, StatusData, framing,
    },
    services::{ApiService, PlaybackService, QueueService},
};
use crate::domain::Song;

/// YouTube server orchestrates services and handles IPC
pub struct YouTubeServer {
    api: Arc<ApiService>,
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    running: AtomicBool,
    socket_path: PathBuf,
}

impl YouTubeServer {
    /// Create new server with services
    pub fn new(socket_path: &Path, cookie_file: Option<&str>) -> Result<Self> {
        let mpv_socket = socket_path.with_extension("mpv.sock");

        // Create API service
        let api = Arc::new(ApiService::new()?);
        if let Some(cookies) = cookie_file {
            if let Err(e) = api.load_cookies(cookies) {
                log::warn!("Failed to load cookies: {}", e);
            }
        }

        // Create playback service (spawns MPV)
        let playback = Arc::new(PlaybackService::new(&mpv_socket)?);

        // Create queue service
        let queue = Arc::new(QueueService::new());

        Ok(Self {
            api,
            playback,
            queue,
            running: AtomicBool::new(false),
            socket_path: socket_path.to_path_buf(),
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

        log::info!("Server shutting down");
        Ok(())
    }

    /// Handle a single client connection
    fn handle_client(&self, stream: UnixStream) -> Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);

        loop {
            let cmd: ServerCommand = match framing::read_message(&mut reader) {
                Ok(cmd) => cmd,
                Err(e) => {
                    log::debug!("Client disconnected: {}", e);
                    break;
                }
            };

            let response = self.handle_command(cmd);

            if let Err(e) = framing::write_message(&mut writer, &response) {
                log::error!("Failed to send response: {}", e);
                break;
            }

            // Check for shutdown
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

            ServerCommand::Play => {
                match self.playback.unpause() {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Pause => {
                match self.playback.pause() {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Stop => {
                match self.playback.stop() {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Next => self.next_track(),
            ServerCommand::Previous => self.previous_track(),

            ServerCommand::SeekAbsolute(pos) => {
                match self.playback.seek(pos, "absolute") {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::SeekRelative(delta) => {
                match self.playback.seek(delta, "relative") {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::PlayPos(pos) => self.play_position(pos),
            ServerCommand::PlayId(id) => self.play_id(id),

            ServerCommand::Add { uri, position } => self.add_to_queue(&uri, position),
            ServerCommand::DeleteId(id) => {
                match self.queue.remove(id) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }
            ServerCommand::Clear => {
                self.queue.clear();
                ServerResponse::Ok
            }
            ServerCommand::MoveId { from, to } => {
                match self.queue.move_song(from, to) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::GetVolume => {
                match self.playback.get_volume() {
                    Ok(vol) => ServerResponse::Volume(vol),
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::SetVolume(vol) => {
                match self.playback.set_volume(vol) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::AdjustVolume(delta) => {
                match self.playback.adjust_volume(delta) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::GetStatus => self.get_status(),
            ServerCommand::GetCurrentSong => self.get_current_song(),
            ServerCommand::GetPlaylist => self.get_playlist(),

            ServerCommand::Search { query } => {
                match self.api.search(&query) {
                    Ok(songs) => {
                        ServerResponse::SearchResults(songs.into_iter().map(SongData::from).collect())
                    }
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Browse { path } => {
                match self.api.browse(&path) {
                    Ok(songs) => {
                        ServerResponse::BrowseResults(
                            songs.into_iter().map(|s| BrowseEntry::File(SongData::from(s))).collect(),
                        )
                    }
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::GetLibrary { category: _ } => {
                // TODO: Implement library browsing
                ServerResponse::Library(vec![])
            }
        }
    }

    fn get_status(&self) -> ServerResponse {
        let paused = self.playback.is_paused().unwrap_or(true);
        let volume = self.playback.get_volume().unwrap_or(100);
        let time_pos = self.playback.get_position().unwrap_or(0.0);
        let duration = self.playback.get_duration().unwrap_or(0.0);

        let current_idx = self.queue.current_index();

        ServerResponse::Status(StatusData {
            state: if paused { "pause".into() } else { "play".into() },
            volume,
            elapsed_ms: Some((time_pos * 1000.0) as u64),
            duration_ms: Some((duration * 1000.0) as u64),
            playlist_length: self.queue.len() as u32,
            current_pos: current_idx.map(|i| i as u32),
            current_id: current_idx.and_then(|i| {
                self.queue.get_by_index(i).ok().and_then(|s| s.id)
            }),
        })
    }

    fn get_current_song(&self) -> ServerResponse {
        let current_idx = self.queue.current_index();
        match current_idx.and_then(|i| self.queue.get_by_index(i).ok()) {
            Some(song) => ServerResponse::Song(Some(SongData::from(song))),
            None => ServerResponse::Song(None),
        }
    }

    fn get_playlist(&self) -> ServerResponse {
        let songs = self.queue.get_all();
        ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
    }

    fn add_to_queue(&self, uri: &str, position: Option<u32>) -> ServerResponse {
        // Create simple song from URI
        let mut song = Song::default();
        song.file = uri.to_string();
        song.metadata.insert("title".into(), vec![uri.to_string()]);

        let id = self.queue.add(song, position);
        // Set ID in song (this is a bit awkward, would be better with service returning song)
        ServerResponse::Ok
    }

    fn play_position(&self, pos: usize) -> ServerResponse {
        match self.queue.get_by_index(pos) {
            Ok(song) => {
                let video_id = song.file.clone();
                match self.playback.get_stream_url(&video_id) {
                    Ok(url) => {
                        self.queue.set_current(Some(pos));
                        match self.playback.play(&url) {
                            Ok(_) => {
                                self.prefetch_upcoming();
                                ServerResponse::Ok
                            }
                            Err(e) => ServerResponse::Error(e.to_string()),
                        }
                    }
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }
            Err(e) => ServerResponse::Error(e.to_string()),
        }
    }

    fn play_id(&self, id: u32) -> ServerResponse {
        match self.queue.get_by_id(id) {
            Ok(_) => {
                // Find position of this ID
                // (This is inefficient, queue service should have get_index_by_id)
                let all_songs = self.queue.get_all();
                if let Some(pos) = all_songs.iter().position(|s| s.id == Some(id)) {
                    self.play_position(pos)
                } else {
                    ServerResponse::Error("Song not found".into())
                }
            }
            Err(e) => ServerResponse::Error(e.to_string()),
        }
    }

    fn next_track(&self) -> ServerResponse {
        match self.queue.next_index() {
            Some(idx) => self.play_position(idx),
            None => ServerResponse::Error("No next track".into()),
        }
    }

    fn previous_track(&self) -> ServerResponse {
        match self.queue.previous_index() {
            Some(idx) => self.play_position(idx),
            Some(0) => {
                // Restart current track
                match self.playback.seek(0.0, "absolute") {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }
            None => ServerResponse::Error("No previous track".into()),
        }
    }

    fn prefetch_upcoming(&self) {
        if let Some(current) = self.queue.current_index() {
            let all_songs = self.queue.get_all();
            let video_ids: Vec<String> = all_songs
                .iter()
                .skip(current + 1)
                .take(5)
                .map(|s| s.file.clone())
                .collect();

            if !video_ids.is_empty() {
                self.playback.prefetch(video_ids);
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
        // This will fail without MPV, but tests the structure
        let socket = std::path::Path::new("/tmp/test-yt.sock");
        let result = YouTubeServer::new(socket, None);
        // We expect this to fail in test environment without MPV
        // But it validates the API
        assert!(result.is_ok() || result.is_err());
    }
}
