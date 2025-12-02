//! YouTube backend server - manages MPV, API, and queue.
//! Can run standalone via `rmpc serve` or be tested independently.

use std::{
    collections::VecDeque,
    io::{BufReader, BufWriter},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;

use super::{
    api::YouTubeApi,
    protocol::{
        BrowseEntry, ServerCommand, ServerResponse, SongData, StatusData, framing,
    },
    stream::StreamExtractor,
};
use crate::{
    domain::{PlaybackState, Song, Status},
    player::mpv_ipc::MpvIpc,
};

/// Queue item with ID
#[derive(Debug, Clone)]
struct QueueItem {
    id: u32,
    song: Song,
}

/// Server state
pub struct YouTubeServer {
    api: YouTubeApi,
    stream_extractor: StreamExtractor,
    mpv: Mutex<MpvIpc>,
    mpv_process: Option<Child>,
    queue: Mutex<VecDeque<QueueItem>>,
    current_idx: Mutex<Option<usize>>,
    next_id: Mutex<u32>,
    running: AtomicBool,
    socket_path: PathBuf,
}

impl YouTubeServer {
    /// Create new server with MPV at given socket path
    pub fn new(socket_path: &Path, cookie_file: Option<&str>) -> Result<Self> {
        let mpv_socket = socket_path.with_extension("mpv.sock");

        // Spawn or connect to MPV
        let (mpv, mpv_process) = Self::connect_or_spawn_mpv(&mpv_socket)?;

        // Initialize YouTube API
        let api = YouTubeApi::new()?;
        if let Some(cookies) = cookie_file {
            if let Err(e) = api.load_cookies(cookies) {
                log::warn!("Failed to load cookies: {}", e);
            }
        }

        Ok(Self {
            api,
            stream_extractor: StreamExtractor::new(),
            mpv: Mutex::new(mpv),
            mpv_process,
            queue: Mutex::new(VecDeque::new()),
            current_idx: Mutex::new(None),
            next_id: Mutex::new(1),
            running: AtomicBool::new(false),
            socket_path: socket_path.to_path_buf(),
        })
    }

    fn connect_or_spawn_mpv(socket_path: &Path) -> Result<(MpvIpc, Option<Child>)> {
        // Try connecting first
        if let Ok(mpv) = MpvIpc::connect(socket_path) {
            log::info!("Connected to existing MPV at {}", socket_path.display());
            return Ok((mpv, None));
        }

        // Spawn MPV
        log::info!("Spawning MPV with socket: {}", socket_path.display());
        let socket_str = socket_path.to_string_lossy();

        let mut child = Command::new("mpv")
            .args([
                "--idle=yes",
                "--vo=null",
                "--no-terminal",
                "--gapless-audio=yes",
                "--prefetch-playlist=yes",
                "--cache=yes",
                "--demuxer-max-bytes=50M",
                "--demuxer-readahead-secs=30",
                "--audio-buffer=1",
                &format!("--input-ipc-server={}", socket_str),
            ])
            .spawn()
            .context("Failed to spawn MPV")?;

        // Wait for socket
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                bail!("Timeout waiting for MPV socket");
            }
            if let Ok(mpv) = MpvIpc::connect(socket_path) {
                log::info!("MPV ready (PID: {})", child.id());
                return Ok((mpv, Some(child)));
            }
            thread::sleep(Duration::from_millis(100));
        }
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
                match self.mpv.lock().set_property("pause", serde_json::json!(false)) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Pause => {
                match self.mpv.lock().set_property("pause", serde_json::json!(true)) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Stop => {
                match self.mpv.lock().send_command(vec!["stop"]) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::Next => self.next_track(),
            ServerCommand::Previous => self.previous_track(),

            ServerCommand::SeekAbsolute(pos) => {
                match self.mpv.lock().send_command(vec!["seek", &pos.to_string(), "absolute"]) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::SeekRelative(delta) => {
                match self.mpv.lock().send_command(vec!["seek", &delta.to_string(), "relative"]) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::PlayPos(pos) => self.play_position(pos),
            ServerCommand::PlayId(id) => self.play_id(id),

            ServerCommand::Add { uri, position } => self.add_to_queue(&uri, position),
            ServerCommand::DeleteId(id) => self.delete_from_queue(id),
            ServerCommand::Clear => self.clear_queue(),
            ServerCommand::MoveId { from, to } => self.move_in_queue(from, to),

            ServerCommand::GetVolume => {
                match self.mpv.lock().get_property("volume") {
                    Ok(v) => {
                        let vol: f64 = serde_json::from_value(v).unwrap_or(100.0);
                        ServerResponse::Volume(vol as u8)
                    }
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::SetVolume(vol) => {
                match self.mpv.lock().set_property("volume", serde_json::json!(vol as f64)) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }

            ServerCommand::AdjustVolume(delta) => {
                let mut mpv = self.mpv.lock();
                match mpv.get_property("volume") {
                    Ok(v) => {
                        let current: f64 = serde_json::from_value(v).unwrap_or(100.0);
                        let new_vol = (current + delta as f64).clamp(0.0, 100.0);
                        match mpv.set_property("volume", serde_json::json!(new_vol)) {
                            Ok(_) => ServerResponse::Ok,
                            Err(e) => ServerResponse::Error(e.to_string()),
                        }
                    }
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
        let mut mpv = self.mpv.lock();

        let paused: bool = mpv
            .get_property("pause")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(true);

        let volume: f64 = mpv
            .get_property("volume")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(100.0);

        let time_pos: f64 = mpv
            .get_property("time-pos")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(0.0);

        let duration: f64 = mpv
            .get_property("duration")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(0.0);

        let queue = self.queue.lock();
        let current_idx = *self.current_idx.lock();

        ServerResponse::Status(StatusData {
            state: if paused { "pause".into() } else { "play".into() },
            volume: volume as u8,
            elapsed_ms: Some((time_pos * 1000.0) as u64),
            duration_ms: Some((duration * 1000.0) as u64),
            playlist_length: queue.len() as u32,
            current_pos: current_idx.map(|i| i as u32),
            current_id: current_idx.and_then(|i| queue.get(i).map(|item| item.id)),
        })
    }

    fn get_current_song(&self) -> ServerResponse {
        let queue = self.queue.lock();
        let current_idx = *self.current_idx.lock();

        match current_idx.and_then(|i| queue.get(i)) {
            Some(item) => ServerResponse::Song(Some(SongData::from(item.song.clone()))),
            None => ServerResponse::Song(None),
        }
    }

    fn get_playlist(&self) -> ServerResponse {
        let queue = self.queue.lock();
        ServerResponse::Playlist(queue.iter().map(|item| SongData::from(item.song.clone())).collect())
    }

    fn add_to_queue(&self, uri: &str, _position: Option<u32>) -> ServerResponse {
        // For now, just add as a simple video ID or search
        let mut song = Song::default();
        song.file = uri.to_string();
        song.metadata.insert("title".into(), vec![uri.to_string()]);

        let mut queue = self.queue.lock();
        let mut next_id = self.next_id.lock();
        let id = *next_id;
        *next_id += 1;

        song.id = Some(id);
        queue.push_back(QueueItem { id, song });

        ServerResponse::Ok
    }

    fn delete_from_queue(&self, id: u32) -> ServerResponse {
        let mut queue = self.queue.lock();
        if let Some(pos) = queue.iter().position(|item| item.id == id) {
            queue.remove(pos);
            ServerResponse::Ok
        } else {
            ServerResponse::Error("Song not found".into())
        }
    }

    fn clear_queue(&self) -> ServerResponse {
        self.queue.lock().clear();
        *self.current_idx.lock() = None;
        ServerResponse::Ok
    }

    fn move_in_queue(&self, from_id: u32, to_id: u32) -> ServerResponse {
        let mut queue = self.queue.lock();
        let from_pos = queue.iter().position(|item| item.id == from_id);
        let to_pos = queue.iter().position(|item| item.id == to_id);

        match (from_pos, to_pos) {
            (Some(from), Some(to)) => {
                if let Some(item) = queue.remove(from) {
                    let insert_pos = to.min(queue.len());
                    queue.insert(insert_pos, item);
                }
                ServerResponse::Ok
            }
            _ => ServerResponse::Error("Song not found".into()),
        }
    }

    fn play_position(&self, pos: usize) -> ServerResponse {
        let queue = self.queue.lock();
        if let Some(item) = queue.get(pos) {
            let video_id = item.song.file.clone();
            drop(queue);

            match self.stream_extractor.get_url(&video_id) {
                Ok(url) => {
                    *self.current_idx.lock() = Some(pos);
                    match self.mpv.lock().send_command(vec!["loadfile", &url, "replace"]) {
                        Ok(_) => {
                            let _ = self.mpv.lock().set_property("pause", serde_json::json!(false));
                            self.prefetch_upcoming();
                            ServerResponse::Ok
                        }
                        Err(e) => ServerResponse::Error(e.to_string()),
                    }
                }
                Err(e) => ServerResponse::Error(e.to_string()),
            }
        } else {
            ServerResponse::Error("Position out of bounds".into())
        }
    }

    fn play_id(&self, id: u32) -> ServerResponse {
        let queue = self.queue.lock();
        if let Some(pos) = queue.iter().position(|item| item.id == id) {
            drop(queue);
            self.play_position(pos)
        } else {
            ServerResponse::Error("Song not found".into())
        }
    }

    fn next_track(&self) -> ServerResponse {
        let current = *self.current_idx.lock();
        let queue_len = self.queue.lock().len();

        match current {
            Some(idx) if idx + 1 < queue_len => self.play_position(idx + 1),
            _ => ServerResponse::Error("No next track".into()),
        }
    }

    fn previous_track(&self) -> ServerResponse {
        let current = *self.current_idx.lock();
        match current {
            Some(idx) if idx > 0 => self.play_position(idx - 1),
            Some(_) => {
                // Restart current track
                match self.mpv.lock().send_command(vec!["seek", "0", "absolute"]) {
                    Ok(_) => ServerResponse::Ok,
                    Err(e) => ServerResponse::Error(e.to_string()),
                }
            }
            None => ServerResponse::Error("No current track".into()),
        }
    }

    fn prefetch_upcoming(&self) {
        let queue = self.queue.lock();
        let current = *self.current_idx.lock();

        if let Some(idx) = current {
            let video_ids: Vec<String> = queue
                .iter()
                .skip(idx + 1)
                .take(5)
                .map(|item| item.song.file.clone())
                .collect();
            drop(queue);

            if !video_ids.is_empty() {
                self.stream_extractor.prefetch(video_ids);
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
        if let Some(mut child) = self.mpv_process.take() {
            log::info!("Killing MPV process");
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_operations() {
        // Create server without MPV (will fail to connect)
        // This tests queue logic only
        let queue: Mutex<VecDeque<QueueItem>> = Mutex::new(VecDeque::new());
        let next_id: Mutex<u32> = Mutex::new(1);

        // Add items
        for i in 0..3 {
            let id = *next_id.lock();
            *next_id.lock() += 1;
            let mut song = Song::default();
            song.file = format!("video{}", i);
            queue.lock().push_back(QueueItem { id, song });
        }

        assert_eq!(queue.lock().len(), 3);

        // Delete item
        let q = queue.lock();
        assert_eq!(q.get(0).unwrap().id, 1);
        assert_eq!(q.get(1).unwrap().id, 2);
        assert_eq!(q.get(2).unwrap().id, 3);
    }
}
