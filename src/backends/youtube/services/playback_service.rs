//! Playback service - manages MPV process and playback control

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossbeam::channel::Sender;
use parking_lot::Mutex;

use super::{super::url_resolver::UrlResolver, InternalEvent};
use crate::backends::youtube::{
    audio::{
        AudioDeliveryPlan, AudioTransportTarget, MpvAudioSource, MpvInput,
        sources::concat::PreparedMediaInputAdapter,
    },
    media::{PreparedMedia, RelayRuntime},
    mpv::{MpvEvent, MpvIpc},
};

/// Observer IDs for MPV properties
const OBSERVER_PLAYLIST_POS: u64 = 1;
const OBSERVER_PAUSE: u64 = 2;
const OBSERVER_IDLE: u64 = 3;
const MPV_SOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MPV_SOCKET_CONNECT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Playback service manages MPV process and URL resolution
pub struct PlaybackService {
    mpv: Mutex<MpvIpc>,
    mpv_process: Option<Child>,
    url_resolver: Arc<UrlResolver>,
    event_loop_running: Arc<AtomicBool>,
    audio_source: Option<Arc<Mutex<dyn MpvAudioSource>>>,
    audio_source_plan: AudioDeliveryPlan,
    relay_runtime: Option<Arc<RelayRuntime>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInputRoute {
    Relay,
    DirectFallback,
    Prepared,
}

#[derive(Debug, Clone)]
pub struct RuntimeInputDecision {
    pub input: MpvInput,
    pub route: RuntimeInputRoute,
}

fn direct_fallback_input(prepared: &PreparedMedia) -> Result<MpvInput> {
    match prepared {
        PreparedMedia::StagedPrefix { url, .. }
        | PreparedMedia::StreamAndCache { url, .. }
        | PreparedMedia::Direct { url } => Ok(MpvInput::new(url.clone())),
        PreparedMedia::LocalFile { .. } => PreparedMediaInputAdapter::build_from_prepared(prepared),
    }
}

fn runtime_input_from_prepared(
    transport: AudioTransportTarget,
    relay_runtime: Option<&RelayRuntime>,
    track_id: &str,
    prepared: &PreparedMedia,
    allow_direct_fallback: bool,
) -> Result<RuntimeInputDecision> {
    match transport {
        AudioTransportTarget::LocalRelay => {
            if matches!(
                prepared,
                PreparedMedia::StagedPrefix { .. } | PreparedMedia::StreamAndCache { .. }
            ) {
                if let Some(relay_runtime) = relay_runtime {
                    match relay_runtime.register_session(track_id, prepared) {
                        Ok(input) => {
                            return Ok(RuntimeInputDecision {
                                input,
                                route: RuntimeInputRoute::Relay,
                            });
                        }
                        Err(error) if allow_direct_fallback => {
                            log::warn!(
                                "relay setup failed for current track {}, falling back to direct: {}",
                                track_id,
                                error
                            );
                            return Ok(RuntimeInputDecision {
                                input: direct_fallback_input(prepared)?,
                                route: RuntimeInputRoute::DirectFallback,
                            });
                        }
                        Err(error) => return Err(error),
                    }
                }

                if allow_direct_fallback {
                    log::warn!(
                        "relay runtime unavailable for current track {}, falling back to direct",
                        track_id
                    );
                    return Ok(RuntimeInputDecision {
                        input: direct_fallback_input(prepared)?,
                        route: RuntimeInputRoute::DirectFallback,
                    });
                }
            }

            Ok(RuntimeInputDecision {
                input: PreparedMediaInputAdapter::build_from_prepared(prepared)?,
                route: RuntimeInputRoute::Prepared,
            })
        }
        AudioTransportTarget::DirectUrl | AudioTransportTarget::PreparedInput => {
            Ok(RuntimeInputDecision {
                input: PreparedMediaInputAdapter::build_from_prepared(prepared)?,
                route: RuntimeInputRoute::Prepared,
            })
        }
    }
}

impl PlaybackService {
    /// Create new playback service, spawning MPV if needed
    pub fn new(
        socket_path: &Path,
        url_resolver: Arc<UrlResolver>,
        audio_source: Option<Arc<Mutex<dyn MpvAudioSource>>>,
        audio_source_plan: AudioDeliveryPlan,
        relay_runtime: Option<Arc<RelayRuntime>>,
    ) -> Result<Self> {
        let (mpv, mpv_process) =
            Self::connect_or_spawn_mpv(socket_path, audio_source_plan.enable_mpv_reconnect)?;

        Ok(Self {
            mpv: Mutex::new(mpv),
            mpv_process,
            url_resolver,
            event_loop_running: Arc::new(AtomicBool::new(false)),
            audio_source,
            audio_source_plan,
            relay_runtime,
        })
    }

    pub fn build_runtime_input(
        &self,
        track_id: &str,
        prepared: &PreparedMedia,
    ) -> Result<MpvInput> {
        Ok(self.build_runtime_input_decision(track_id, prepared)?.input)
    }

    pub fn build_runtime_input_decision(
        &self,
        track_id: &str,
        prepared: &PreparedMedia,
    ) -> Result<RuntimeInputDecision> {
        runtime_input_from_prepared(
            self.audio_source_plan.transport,
            self.relay_runtime.as_deref(),
            track_id,
            prepared,
            false,
        )
    }

    pub fn build_current_runtime_input_with_direct_fallback(
        &self,
        track_id: &str,
        prepared: &PreparedMedia,
    ) -> Result<RuntimeInputDecision> {
        runtime_input_from_prepared(
            self.audio_source_plan.transport,
            self.relay_runtime.as_deref(),
            track_id,
            prepared,
            true,
        )
    }

    /// Start the MPV event loop in a background thread
    ///
    /// This sets up property observation and spawns a thread that reads MPV
    /// events. Events are sent via the provided channel.
    ///
    /// # Arguments
    /// * `socket_path` - Path to the MPV socket (for creating dedicated event
    ///   reader)
    /// * `event_tx` - Channel sender for broadcasting events ("player",
    ///   "playlist")
    pub fn start_event_loop(
        &self,
        socket_path: &Path,
        event_tx: Sender<String>,
        internal_event_tx: Sender<InternalEvent>,
    ) -> Result<()> {
        if self.event_loop_running.swap(true, Ordering::SeqCst) {
            log::warn!("Event loop already running");
            return Ok(());
        }

        let mut event_reader =
            MpvIpc::connect(socket_path).context("Failed to create event reader connection")?;

        event_reader.observe_property(OBSERVER_PLAYLIST_POS, "playlist-pos")?;
        event_reader.observe_property(OBSERVER_PAUSE, "pause")?;
        event_reader.observe_property(OBSERVER_IDLE, "idle-active")?;
        log::info!("MPV property observation set up on event reader connection");

        let running = self.event_loop_running.clone();

        // Spawn event loop thread
        thread::spawn(move || {
            log::info!("MPV event loop started");

            while running.load(Ordering::SeqCst) {
                match event_reader.read_event() {
                    Ok(Some(event)) => {
                        match event {
                            MpvEvent::TrackChanged { position } => {
                                log::info!("Track changed to position {}", position);
                                let position =
                                    position.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                                // Send to internal processor only - it will emit "player" after
                                // queue sync
                                let _ = internal_event_tx
                                    .send(InternalEvent::TrackChanged { position });
                            }
                            MpvEvent::PauseChanged { paused } => {
                                log::debug!("Pause state: {}", paused);
                                let _ = event_tx.send("player".to_string());
                            }
                            MpvEvent::EndFile { reason, file_error } => {
                                if let Some(ref err) = file_error {
                                    log::warn!(
                                        "Track ended with error: reason={}, file_error={}",
                                        reason,
                                        err
                                    );
                                } else {
                                    log::info!("Track ended: {}", reason);
                                }
                                // Send to internal processor only - it will emit "player" after
                                // queue sync
                                let _ = internal_event_tx
                                    .send(InternalEvent::EndFile { reason: reason.clone() });
                            }
                            MpvEvent::IdleChanged { idle } => {
                                log::debug!("Idle state: {}", idle);
                                let _ = internal_event_tx.send(InternalEvent::IdleChanged { idle });
                                if idle {
                                    let _ = event_tx.send("player".to_string());
                                }
                            }
                            MpvEvent::PlaybackStarted => {
                                let _ = internal_event_tx.send(InternalEvent::PlaybackStarted);
                            }
                            MpvEvent::Other(_) => {
                                // Ignore other events
                            }
                        }
                    }
                    Ok(None) => {
                        // Timeout - no events, just continue checking running
                        // flag The 500ms timeout in
                        // MpvIpc prevents busy-spinning
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
    fn connect_or_spawn_mpv(
        socket_path: &Path,
        enable_reconnect: bool,
    ) -> Result<(MpvIpc, Option<Child>)> {
        log::info!("Checking for existing MPV at socket: {}", socket_path.display());

        if let Some(mpv) = Self::try_connect_existing_mpv(socket_path)? {
            log::info!(
                "✓ Connected to EXISTING MPV at {} (reusing process)",
                socket_path.display()
            );
            log::warn!("⚠ Note: MPV may have old playlist state from previous session");
            return Ok((mpv, None));
        }

        Self::cleanup_stale_socket(socket_path)?;

        log::info!("Spawning NEW MPV process with socket: {}", socket_path.display());
        let args = Self::build_mpv_args(socket_path, enable_reconnect);
        let mut child = Command::new("mpv").args(&args).spawn().context("Failed to spawn MPV")?;
        let mpv = Self::wait_for_spawned_mpv(socket_path, &mut child)?;

        log::info!("✓ NEW MPV ready (PID: {}) - fresh playlist state", child.id());
        Ok((mpv, Some(child)))
    }

    fn try_connect_existing_mpv(socket_path: &Path) -> Result<Option<MpvIpc>> {
        if !socket_path.exists() {
            log::info!("No socket file found at {}", socket_path.display());
            return Ok(None);
        }

        log::info!("Socket file exists, attempting connection...");
        match MpvIpc::connect(socket_path) {
            Ok(mpv) => Ok(Some(mpv)),
            Err(error) => {
                log::warn!("Socket exists but connection failed ({}) - treating as stale", error);
                Ok(None)
            }
        }
    }

    fn cleanup_stale_socket(socket_path: &Path) -> Result<()> {
        if !socket_path.exists() {
            return Ok(());
        }

        fs::remove_file(socket_path).with_context(|| {
            format!("Failed to remove stale MPV socket {}", socket_path.display())
        })?;

        log::info!("Removed stale MPV socket: {}", socket_path.display());
        Ok(())
    }

    fn build_mpv_args(socket_path: &Path, enable_reconnect: bool) -> Vec<String> {
        let socket_str = socket_path.to_string_lossy();
        let mut args = vec![
            "--no-config".to_string(),
            "--ytdl=no".to_string(),
            "--idle=yes".to_string(),
            "--vo=null".to_string(),
            "--no-terminal".to_string(),
            "--gapless-audio=yes".to_string(),
            "--prefetch-playlist=yes".to_string(),
            "--cache=yes".to_string(),
            "--cache-secs=60".to_string(),
            "--cache-pause=yes".to_string(),
            "--cache-pause-initial=yes".to_string(),
            "--demuxer-max-bytes=50M".to_string(),
            "--demuxer-readahead-secs=30".to_string(),
            "--audio-buffer=1".to_string(),
            "--audio-client-name=music-daemon".to_string(),
            "--stream-lavf-o-append=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat"
                .to_string(),
            "--script=/usr/lib/mpv-mpris/mpris.so".to_string(),
            format!("--input-ipc-server={}", socket_str),
        ];

        if enable_reconnect {
            args.push("--stream-lavf-o-append=reconnect=1".to_string());
            args.push("--stream-lavf-o-append=reconnect_streamed=1".to_string());
            args.push("--stream-lavf-o-append=reconnect_at_eof=1".to_string());
            args.push("--stream-lavf-o-append=reconnect_delay_max=5".to_string());
        }

        if log::log_enabled!(log::Level::Debug) {
            args.push("--log-file=/tmp/mpv-debug.log".to_string());
            args.push("--msg-level=all=v".to_string());
            log::trace!("MPV verbose logging enabled: /tmp/mpv-debug.log");
        }

        args
    }

    fn wait_for_spawned_mpv(socket_path: &Path, child: &mut Child) -> Result<MpvIpc> {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().context("Failed to poll MPV process")? {
                bail!("MPV exited before socket was ready (status: {})", status);
            }

            if let Ok(mpv) = MpvIpc::connect(socket_path) {
                return Ok(mpv);
            }

            if start.elapsed() > MPV_SOCKET_CONNECT_TIMEOUT {
                let _ = child.kill();
                bail!("Timeout waiting for MPV socket");
            }

            thread::sleep(MPV_SOCKET_CONNECT_POLL_INTERVAL);
        }
    }

    /// Play URL with metadata
    /// Sets force-media-title BEFORE loadfile so MPRIS shows proper metadata
    pub fn play(&self, url: &str, title: &str, artist: &str) -> Result<()> {
        // MPRIS fix: Set force-media-title before loadfile
        // This ensures MPRIS (playerctl, KDE Connect, etc.) shows song title instead of
        // URL
        let media_title =
            if artist.is_empty() { title.to_string() } else { format!("{} - {}", artist, title) };

        log::debug!("Setting force-media-title to: {}", media_title);
        self.mpv.lock().set_property("force-media-title", serde_json::json!(media_title))?;

        self.mpv.lock().send_command(vec!["loadfile", url, "replace"])?;
        self.mpv.lock().set_property("pause", serde_json::json!(false))?;
        Ok(())
    }

    /// Play using MpvInput. Sets mpv_args as properties before loadfile.
    pub fn play_with_input(&self, input: &MpvInput, title: &str, artist: &str) -> Result<()> {
        let media_title =
            if artist.is_empty() { title.to_string() } else { format!("{} - {}", artist, title) };

        let mode = Self::mode_label(self.audio_source_plan.transport);
        let url_preview = if input.url.len() > 80 { &input.url[..80] } else { &input.url };

        log::info!(
            "[PLAYBACK] mode={} title=\"{}\" url_preview=\"{}...\"",
            mode,
            media_title,
            url_preview
        );

        log::debug!("Setting force-media-title to: {}", media_title);
        self.mpv.lock().set_property("force-media-title", serde_json::json!(media_title))?;

        self.apply_mpv_args(&input.mpv_args)?;

        log::info!("[PLAYBACK] loadfile: {}", &input.url);
        self.mpv.lock().send_command(vec!["loadfile", &input.url, "replace"])?;

        self.mpv.lock().set_property("pause", serde_json::json!(false))?;
        log::info!("[PLAYBACK] started mode={}", mode);
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

    fn mode_label(transport: AudioTransportTarget) -> &'static str {
        match transport {
            AudioTransportTarget::DirectUrl => "DIRECT",
            AudioTransportTarget::PreparedInput => "STAGED",
            AudioTransportTarget::LocalRelay => "RELAY",
        }
    }

    /// Prefetch stream URLs for upcoming videos
    pub fn prefetch(&self, video_ids: Vec<String>) {
        self.url_resolver.prefetch(video_ids);
    }

    pub fn prefetch_audio_batch(&self, video_ids: Vec<String>) {
        self.prefetch(video_ids);
    }

    pub fn has_cached_audio(&self, video_id: &str) -> bool {
        if let Some(ref source) = self.audio_source {
            source.lock().has_cached(video_id)
        } else {
            false
        }
    }

    // ========== MPV Playlist Commands ==========
    // These commands manage MPV's internal playlist, allowing it to be the
    // single source of truth for playback order.

    /// Append URL to MPV's playlist without starting playback.
    /// Use this instead of `play()` when building a queue.
    pub fn playlist_append(&self, url: &str) -> Result<()> {
        let url_preview = if url.len() > 100 { &url[..100] } else { url };
        log::info!("[PLAYBACK] playlist_append url=\"{}...\"", url_preview);
        self.mpv.lock().send_command(vec!["loadfile", url, "append"])?;
        Ok(())
    }

    pub fn playlist_append_input(&self, input: &MpvInput) -> Result<()> {
        self.apply_mpv_args(&input.mpv_args)?;

        let url_preview = if input.url.len() > 100 { &input.url[..100] } else { &input.url };
        log::info!("[PLAYBACK] playlist_append_input url=\"{}...\"", url_preview);
        self.mpv.lock().send_command(vec!["loadfile", &input.url, "append"])?;
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
        let media_title =
            if artist.is_empty() { title.to_string() } else { format!("{} - {}", artist, title) };
        self.mpv.lock().set_property("force-media-title", serde_json::json!(media_title))?;
        Ok(())
    }

    fn apply_mpv_args(&self, mpv_args: &[String]) -> Result<()> {
        for arg in mpv_args {
            if let Some((key, value)) = runtime_property_update_from_arg(arg) {
                log::debug!("[PLAYBACK] mpv_property: {}={}", key, value);
                self.mpv.lock().set_property(key, serde_json::json!(value))?;
            }
        }

        Ok(())
    }
}

fn runtime_property_update_from_arg(arg: &str) -> Option<(&str, &str)> {
    let opt = arg.strip_prefix("--")?;
    let (key, value) = opt.split_once('=')?;

    if key == "stream-lavf-o-append" {
        return None;
    }

    Some((key, value))
}

impl Drop for PlaybackService {
    fn drop(&mut self) {
        self.event_loop_running.store(false, Ordering::SeqCst);

        if let Some(mut child) = self.mpv_process.take() {
            log::info!("Killing MPV process");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{RuntimeInputRoute, runtime_input_from_prepared};
    use crate::backends::youtube::{
        audio::AudioTransportTarget,
        media::{PreparedMedia, RelayRuntime},
    };

    use super::runtime_property_update_from_arg;

    #[test]
    fn runtime_property_update_ignores_stream_lavf_append_flags() {
        assert_eq!(
            runtime_property_update_from_arg(
                "--stream-lavf-o-append=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat"
            ),
            None
        );
    }

    #[test]
    fn runtime_property_update_keeps_regular_property_flags() {
        assert_eq!(runtime_property_update_from_arg("--pause=yes"), Some(("pause", "yes")));
    }

    #[test]
    fn relay_current_track_falls_back_to_direct_when_runtime_is_unavailable() {
        let prepared = PreparedMedia::StreamAndCache {
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 1024,
        };

        let decision = runtime_input_from_prepared(
            AudioTransportTarget::LocalRelay,
            None,
            "track-123",
            &prepared,
            true,
        )
        .unwrap();

        assert_eq!(decision.route, RuntimeInputRoute::DirectFallback);
        assert_eq!(decision.input.url, "https://example.com/upstream");
        assert!(decision.input.mpv_args.is_empty());
    }

    #[test]
    fn relay_current_track_keeps_relay_path_when_runtime_is_available() {
        let relay_runtime = RelayRuntime::start().unwrap();
        let prepared = PreparedMedia::StreamAndCache {
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 1024,
        };

        let decision = runtime_input_from_prepared(
            AudioTransportTarget::LocalRelay,
            Some(&relay_runtime),
            "track-123",
            &prepared,
            true,
        )
        .unwrap();

        assert_eq!(decision.route, RuntimeInputRoute::Relay);
        assert!(decision.input.url.contains("/relay/"));
    }
}
