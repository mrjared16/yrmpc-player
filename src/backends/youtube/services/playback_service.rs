//! Playback service - manages MPV process and playback control

use anyhow::{Context, Result, bail};
use crossbeam::channel::Sender;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use crate::backends::youtube::mpv::{MpvIpc, MpvEvent};
use super::super::url_resolver::UrlResolver;
use super::super::config::ExtractorType;
use super::super::audio_cache::AudioCache;

/// Observer IDs for MPV properties
const OBSERVER_PLAYLIST_POS: u64 = 1;
const OBSERVER_PAUSE: u64 = 2;
const OBSERVER_IDLE: u64 = 3;

/// Default prefetch duration for audio cache (seconds)
const DEFAULT_AUDIO_PREFETCH_SECS: f64 = 10.0;

/// Playback service manages MPV process and URL resolution
pub struct PlaybackService {
    mpv: Mutex<MpvIpc>,
    mpv_process: Option<Child>,
    url_resolver: UrlResolver,
    audio_cache: Option<AudioCache>,
    event_loop_running: Arc<AtomicBool>,
}

impl PlaybackService {
    /// Create new playback service, spawning MPV if needed
    pub fn new(socket_path: &Path, extractor_type: ExtractorType) -> Result<Self> {
        let (mpv, mpv_process) = Self::connect_or_spawn_mpv(socket_path)?;

        // Initialize audio cache for instant playback
        // This is optional - if it fails, we fall back to direct URLs
        let audio_cache = match AudioCache::new() {
            Ok(cache) => {
                log::info!("Audio cache initialized: {:?}", cache.stats().cache_dir);
                Some(cache)
            }
            Err(e) => {
                log::warn!("Failed to initialize audio cache, using direct URLs: {}", e);
                None
            }
        };

        Ok(Self {
            mpv: Mutex::new(mpv),
            mpv_process,
            url_resolver: UrlResolver::new(extractor_type),
            audio_cache,
            event_loop_running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Start the MPV event loop in a background thread
    ///
    /// This sets up property observation and spawns a thread that reads MPV events.
    /// Events are sent via the provided channel.
    ///
    /// # Arguments
    /// * `socket_path` - Path to the MPV socket (for creating dedicated event reader)
    /// * `event_tx` - Channel sender for broadcasting events ("player", "playlist")
    pub fn start_event_loop(&self, socket_path: &Path, event_tx: Sender<String>) -> Result<()> {
        // Don't start if already running
        if self.event_loop_running.swap(true, Ordering::SeqCst) {
            log::warn!("Event loop already running");
            return Ok(());
        }

        // Set up property observation on main connection
        {
            let mut mpv = self.mpv.lock();
            mpv.observe_property(OBSERVER_PLAYLIST_POS, "playlist-pos")?;
            mpv.observe_property(OBSERVER_PAUSE, "pause")?;
            mpv.observe_property(OBSERVER_IDLE, "idle-active")?;
            log::info!("MPV property observation set up");
        }

        // Create dedicated connection for event reading
        let mut event_reader = MpvIpc::connect(socket_path)
            .context("Failed to create event reader connection")?;

        let running = self.event_loop_running.clone();

        // Spawn event loop thread
        thread::spawn(move || {
            log::info!("MPV event loop started");

            while running.load(Ordering::SeqCst) {
                match event_reader.read_event() {
                    Ok(Some(event)) => {
                        log::debug!("MPV event: {:?}", event);

                        match event {
                            MpvEvent::TrackChanged { position } => {
                                log::info!("Track changed to position {}", position);
                                let _ = event_tx.send("player".to_string());
                            }
                            MpvEvent::PauseChanged { paused } => {
                                log::debug!("Pause state: {}", paused);
                                let _ = event_tx.send("player".to_string());
                            }
                            MpvEvent::EndFile { reason } => {
                                log::info!("Track ended: {}", reason);
                                // Send special event for end-of-file handling
                                let _ = event_tx.send(format!("end-file:{}", reason));
                            }
                            MpvEvent::IdleChanged { idle } => {
                                log::debug!("Idle state: {}", idle);
                                if idle {
                                    let _ = event_tx.send("player".to_string());
                                }
                            }
                            MpvEvent::Other(_) => {
                                // Ignore other events
                            }
                        }
                    }
                    Ok(None) => {
                        // Timeout - no events, just continue checking running flag
                        // The 500ms timeout in MpvIpc prevents busy-spinning
                    }
                    Err(e) => {
                        if running.load(Ordering::SeqCst) {
                            log::error!("Event loop error: {}", e);
                            // Brief pause before retrying
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            }

            log::info!("MPV event loop stopped");
        });

        Ok(())
    }

    /// Stop the event loop
    pub fn stop_event_loop(&self) {
        self.event_loop_running.store(false, Ordering::SeqCst);
    }

    /// Connect to existing MPV or spawn new process
    fn connect_or_spawn_mpv(socket_path: &Path) -> Result<(MpvIpc, Option<Child>)> {
        // Try connecting to existing MPV first
        log::info!("Checking for existing MPV at socket: {}", socket_path.display());

        if socket_path.exists() {
            log::info!("Socket file exists, attempting connection...");
            if let Ok(mpv) = MpvIpc::connect(socket_path) {
                log::info!("✓ Connected to EXISTING MPV at {} (reusing process)", socket_path.display());
                log::warn!("⚠ Note: MPV may have old playlist state from previous session");
                return Ok((mpv, None));
            } else {
                log::warn!("Socket exists but connection failed - MPV may have crashed");
            }
        } else {
            log::info!("No socket file found at {}", socket_path.display());
        }

        // Spawn new MPV process
        log::info!("Spawning NEW MPV process with socket: {}", socket_path.display());
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

        // Wait for socket to become available
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                bail!("Timeout waiting for MPV socket");
            }
            if let Ok(mpv) = MpvIpc::connect(socket_path) {
                log::info!("✓ NEW MPV ready (PID: {}) - fresh playlist state", child.id());
                return Ok((mpv, Some(child)));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Play URL with metadata
    /// Sets force-media-title BEFORE loadfile so MPRIS shows proper metadata
    pub fn play(&self, url: &str, title: &str, artist: &str) -> Result<()> {
        // MPRIS fix: Set force-media-title before loadfile
        // This ensures MPRIS (playerctl, KDE Connect, etc.) shows song title instead of URL
        let media_title = if artist.is_empty() {
            title.to_string()
        } else {
            format!("{} - {}", artist, title)
        };
        
        log::debug!("Setting force-media-title to: {}", media_title);
        self.mpv.lock().set_property("force-media-title", serde_json::json!(media_title))?;
        
        self.mpv.lock().send_command(vec!["loadfile", url, "replace"])?;
        self.mpv.lock().set_property("pause", serde_json::json!(false))?;
        Ok(())
    }


    /// Pause playback
    pub fn pause(&self) -> Result<()> {
        self.mpv.lock().set_property("pause", serde_json::json!(true))?;
        Ok(())
    }

    /// Unpause/resume playback
    pub fn unpause(&self) -> Result<()> {
        self.mpv.lock().set_property("pause", serde_json::json!(false))?;
        Ok(())
    }

    /// Stop playback
    pub fn stop(&self) -> Result<()> {
        self.mpv.lock().send_command(vec!["stop"])?;
        Ok(())
    }

    /// Seek to position
    pub fn seek(&self, position: f64, mode: &str) -> Result<()> {
        self.mpv.lock().send_command(vec!["seek", &position.to_string(), mode])?;
        Ok(())
    }

    /// Get current playback position
    pub fn get_position(&self) -> Result<f64> {
        let val = self.mpv.lock().get_property("time-pos")?;
        serde_json::from_value(val).map_err(Into::into)
    }

    /// Get track duration
    pub fn get_duration(&self) -> Result<f64> {
        let val = self.mpv.lock().get_property("duration")?;
        serde_json::from_value(val).map_err(Into::into)
    }

    /// Check if paused
    pub fn is_paused(&self) -> Result<bool> {
        let val = self.mpv.lock().get_property("pause")?;
        serde_json::from_value(val).map_err(Into::into)
    }

    /// Get volume (0-100)
    pub fn get_volume(&self) -> Result<u8> {
        let val = self.mpv.lock().get_property("volume")?;
        let vol: f64 = serde_json::from_value(val)?;
        Ok(vol as u8)
    }

    /// Set volume (0-100)
    pub fn set_volume(&self, volume: u8) -> Result<()> {
        self.mpv.lock().set_property("volume", serde_json::json!(volume as f64))?;
        Ok(())
    }

    /// Adjust volume by delta
    pub fn adjust_volume(&self, delta: i8) -> Result<()> {
        let current = self.get_volume()?;
        let new_vol = ((current as i16) + (delta as i16)).clamp(0, 100) as u8;
        self.set_volume(new_vol)
    }

    /// Get stream URL for video ID
    pub fn get_stream_url(&self, video_id: &str) -> Result<String> {
        self.url_resolver.get_url(video_id)
    }

    /// Prefetch stream URLs for upcoming videos
    pub fn prefetch(&self, video_ids: Vec<String>) {
        self.url_resolver.prefetch(video_ids);
    }

    // ========== MPV Playlist Commands ==========
    // These commands manage MPV's internal playlist, allowing it to be the
    // single source of truth for playback order.

    /// Append URL to MPV's playlist without starting playback.
    /// Use this instead of `play()` when building a queue.
    pub fn playlist_append(&self, url: &str) -> Result<()> {
        self.mpv.lock().send_command(vec!["loadfile", url, "append"])?;
        Ok(())
    }

    /// Append URL and start playing it immediately.
    /// Equivalent to adding to playlist then playing the new entry.
    pub fn playlist_append_play(&self, url: &str) -> Result<()> {
        self.mpv.lock().send_command(vec!["loadfile", url, "append-play"])?;
        Ok(())
    }

    /// Remove track at index from MPV's playlist.
    /// If removing the currently playing track, MPV will stop or advance.
    pub fn playlist_remove(&self, idx: usize) -> Result<()> {
        self.mpv.lock().send_command(vec!["playlist-remove", &idx.to_string()])?;
        Ok(())
    }

    /// Move track in MPV's playlist from one index to another.
    pub fn playlist_move(&self, from: usize, to: usize) -> Result<()> {
        self.mpv.lock().send_command(vec!["playlist-move", &from.to_string(), &to.to_string()])?;
        Ok(())
    }

    /// Clear MPV's entire playlist.
    pub fn playlist_clear(&self) -> Result<()> {
        self.mpv.lock().send_command(vec!["playlist-clear"])?;
        Ok(())
    }

    /// Play track at specific index in MPV's playlist.
    /// This is the proper way to switch tracks when using playlist mode.
    pub fn playlist_play_index(&self, idx: usize) -> Result<()> {
        self.mpv.lock().send_command(vec!["playlist-play-index", &idx.to_string()])?;
        Ok(())
    }

    /// Get current playlist position (-1 if nothing playing).
    pub fn get_playlist_pos(&self) -> Result<i64> {
        let val = self.mpv.lock().get_property("playlist-pos")?;
        serde_json::from_value(val).map_err(Into::into)
    }

    /// Get total number of items in MPV's playlist.
    pub fn get_playlist_count(&self) -> Result<usize> {
        let val = self.mpv.lock().get_property("playlist-count")?;
        let count: i64 = serde_json::from_value(val)?;
        Ok(count.max(0) as usize)
    }

    /// Set the media title that MPRIS will display.
    /// Call this before `playlist_play_index()` to ensure proper metadata.
    pub fn set_media_title(&self, title: &str, artist: &str) -> Result<()> {
        let media_title = if artist.is_empty() {
            title.to_string()
        } else {
            format!("{} - {}", artist, title)
        };
        self.mpv.lock().set_property("force-media-title", serde_json::json!(media_title))?;
        Ok(())
    }

    // ========== Audio Cache & EDL URL Building ==========

    /// Build optimal playback URL for a video ID.
    ///
    /// If audio cache has the first 10s cached, builds an EDL URL for instant playback:
    /// - First 10s from local cache (instant, no network latency)
    /// - Remainder from network stream (seamless transition)
    ///
    /// If no cache, returns the direct stream URL.
    pub fn build_playback_url(&self, video_id: &str) -> Result<String> {
        // First, resolve the stream URL (may be cached)
        let stream_url = self.url_resolver.get_url(video_id)?;

        // Check if we have cached audio
        if let Some(ref cache) = self.audio_cache {
            if let Some(cache_path) = cache.get_path(video_id) {
                let cached_duration = cache.get_cached_duration(video_id).unwrap_or(DEFAULT_AUDIO_PREFETCH_SECS);

                // Build EDL URL: local cache first, then network
                let edl_url = format!(
                    "edl://{},0,{};{},{},",
                    cache_path.display(),
                    cached_duration,
                    stream_url,
                    cached_duration
                );

                log::debug!("Built EDL URL for {}: cache {}s + network", video_id, cached_duration);
                return Ok(edl_url);
            }
        }

        // No cache - return direct URL
        log::debug!("No audio cache for {}, using direct URL", video_id);
        Ok(stream_url)
    }

    /// Prefetch audio data for a video ID.
    ///
    /// Downloads the first N seconds of audio and caches to disk.
    /// Call this after URL resolution to prepare for instant playback.
    pub fn prefetch_audio(&self, video_id: &str, stream_url: &str) {
        if let Some(ref cache) = self.audio_cache {
            let video_id = video_id.to_string();
            let stream_url = stream_url.to_string();

            // Check if already cached
            if cache.has(&video_id) {
                log::debug!("Audio already cached for {}", video_id);
                return;
            }

            // Prefetch in background
            cache.prefetch_async(video_id, stream_url);
        }
    }

    /// Prefetch audio for multiple video IDs.
    ///
    /// Resolves URLs and downloads first N seconds for each.
    pub fn prefetch_audio_batch(&self, video_ids: Vec<String>) {
        if self.audio_cache.is_none() {
            return;
        }

        // First resolve URLs
        let url_results = self.url_resolver.get_urls(&video_ids);

        // Then prefetch audio for successful resolutions
        for (video_id, url_result) in url_results {
            if let Ok(url) = url_result {
                self.prefetch_audio(&video_id, &url);
            }
        }
    }

    /// Check if audio is cached for a video ID.
    pub fn has_cached_audio(&self, video_id: &str) -> bool {
        self.audio_cache.as_ref().map_or(false, |c| c.has(video_id))
    }

    /// Get audio cache statistics.
    pub fn audio_cache_stats(&self) -> Option<super::super::audio_cache::AudioCacheStats> {
        self.audio_cache.as_ref().map(|c| c.stats())
    }
}

impl Drop for PlaybackService {
    fn drop(&mut self) {
        if let Some(mut child) = self.mpv_process.take() {
            log::info!("Killing MPV process");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
