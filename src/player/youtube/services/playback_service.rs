//! Playback service - manages MPV process and playback control

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;
use crate::player::mpv_ipc::MpvIpc;
use super::super::stream::StreamExtractor;

/// Playback service manages MPV process and stream extraction
pub struct PlaybackService {
    mpv: Mutex<MpvIpc>,
    mpv_process: Option<Child>,
    stream_extractor: StreamExtractor,
}

impl PlaybackService {
    /// Create new playback service, spawning MPV if needed
    pub fn new(socket_path: &Path) -> Result<Self> {
        let (mpv, mpv_process) = Self::connect_or_spawn_mpv(socket_path)?;
        
        Ok(Self {
            mpv: Mutex::new(mpv),
            mpv_process,
            stream_extractor: StreamExtractor::new(),
        })
    }

    /// Connect to existing MPV or spawn new process
    fn connect_or_spawn_mpv(socket_path: &Path) -> Result<(MpvIpc, Option<Child>)> {
        // Try connecting to existing MPV first
        if let Ok(mpv) = MpvIpc::connect(socket_path) {
            log::info!("Connected to existing MPV at {}", socket_path.display());
            return Ok((mpv, None));
        }

        // Spawn new MPV process
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

        // Wait for socket to become available
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

    /// Play URL with metadata
    pub fn play(&self, url: &str, _title: &str, _artist: &str) -> Result<()> {
        self.mpv.lock().send_command(vec!["loadfile", url, "replace"])?;
        // TODO: MPRIS metadata - media-title property access fails
        // Need to investigate correct MPV property or use script-message
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
        self.stream_extractor.get_url(video_id)
    }

    /// Prefetch stream URLs for upcoming videos
    pub fn prefetch(&self, video_ids: Vec<String>) {
        self.stream_extractor.prefetch(video_ids);
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
