use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use parking_lot::Mutex;

use super::{backend::MusicBackend, mpv_ipc::MpvIpc};
use crate::mpd::commands::*;

/// MPV backend implementation
///
/// This implements the MusicBackend trait using MPV's JSON IPC protocol.
/// Since MPV doesn't have a built-in queue, we manage it internally.
pub struct MpvBackend {
    ipc: Arc<Mutex<MpvIpc>>,
    queue: Arc<Mutex<Vec<Song>>>,
    current_index: Arc<Mutex<usize>>,
}

impl MpvBackend {
    pub fn new<P: AsRef<std::path::Path>>(socket_path: P) -> Result<Self> {
        let ipc = MpvIpc::connect(socket_path)?;

        Ok(Self {
            ipc: Arc::new(Mutex::new(ipc)),
            queue: Arc::new(Mutex::new(Vec::new())),
            current_index: Arc::new(Mutex::new(0)),
        })
    }
}

impl MusicBackend for MpvBackend {
    // ===== Playback Control =====

    fn play(&mut self) -> Result<()> {
        let mut ipc = self.ipc.lock();
        ipc.set_property("pause", serde_json::json!(false))
    }

    fn pause(&mut self, state: bool) -> Result<()> {
        let mut ipc = self.ipc.lock();
        ipc.set_property("pause", serde_json::json!(state))
    }

    fn stop(&mut self) -> Result<()> {
        let mut ipc = self.ipc.lock();
        ipc.send_command(vec!["stop"])?;
        Ok(())
    }

    fn next(&mut self) -> Result<()> {
        let mut idx = self.current_index.lock();
        let queue = self.queue.lock();

        if *idx + 1 < queue.len() {
            *idx += 1;
            if let Some(song) = queue.get(*idx) {
                let mut ipc = self.ipc.lock();
                ipc.send_command(vec!["loadfile", &song.file])?;
            }
        }
        Ok(())
    }

    fn previous(&mut self) -> Result<()> {
        let mut idx = self.current_index.lock();

        if *idx > 0 {
            *idx -= 1;
            let queue = self.queue.lock();
            if let Some(song) = queue.get(*idx) {
                let mut ipc = self.ipc.lock();
                ipc.send_command(vec!["loadfile", &song.file])?;
            }
        }
        Ok(())
    }

    fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        let mut ipc = self.ipc.lock();

        match position {
            SeekPosition::Absolute(secs) => {
                ipc.send_command(vec!["seek", &secs.to_string(), "absolute"])?;
            }
            SeekPosition::Relative(secs) => {
                ipc.send_command(vec!["seek", &secs.to_string(), "relative"])?;
            }
        }
        Ok(())
    }

    // ===== Status Queries =====

    fn get_status(&mut self) -> Result<Status> {
        let mut ipc = self.ipc.lock();

        let paused: bool =
            serde_json::from_value(ipc.get_property("pause").unwrap_or(serde_json::json!(true)))
                .unwrap_or(true);

        let idle: bool = serde_json::from_value(
            ipc.get_property("idle-active").unwrap_or(serde_json::json!(true)),
        )
        .unwrap_or(true);

        let time_pos: f64 =
            serde_json::from_value(ipc.get_property("time-pos").unwrap_or(serde_json::json!(0.0)))
                .unwrap_or(0.0);

        let duration: f64 =
            serde_json::from_value(ipc.get_property("duration").unwrap_or(serde_json::json!(0.0)))
                .unwrap_or(0.0);

        let volume: f64 =
            serde_json::from_value(ipc.get_property("volume").unwrap_or(serde_json::json!(100.0)))
                .unwrap_or(100.0);

        let state = if idle {
            State::Stop
        } else if paused {
            State::Pause
        } else {
            State::Play
        };

        Ok(Status {
            state,
            volume: volume as u8,
            repeat: false, // MPV doesn't have these concepts
            random: false,
            single: OnOffOneshot::Off,
            consume: OnOffOneshot::Off,
            playlist_length: self.queue.lock().len(),
            song: *self.current_index.lock(),
            elapsed: Some(std::time::Duration::from_secs_f64(time_pos)),
            duration: Some(std::time::Duration::from_secs_f64(duration)),
            crossfade: None,
            bitrate: None,
            audio_format: None,
            updating_db: None,
        })
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        Ok(self.queue.lock().clone())
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        let queue = self.queue.lock();
        let idx = *self.current_index.lock();
        Ok(queue.get(idx).cloned())
    }

    // ===== Queue Management =====

    fn add(&mut self, uri: &str, _position: Option<QueuePosition>) -> Result<()> {
        // Create a minimal Song struct
        let song = Song {
            file: uri.to_string(),
            title: None,
            artist: None,
            album: None,
            duration: None,
            id: self.queue.lock().len() as u32,
            last_modified: chrono::Utc::now(),
            added: Some(chrono::Utc::now()),
            // ... other fields with defaults
            ..Default::default()
        };

        self.queue.lock().push(song);
        Ok(())
    }

    fn delete_id(&mut self, id: u32) -> Result<()> {
        let mut queue = self.queue.lock();
        queue.retain(|s| s.id != id);
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.queue.lock().clear();
        *self.current_index.lock() = 0;
        Ok(())
    }

    fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        // Simplified implementation
        let mut queue = self.queue.lock();
        if let Some(from_idx) = queue.iter().position(|s| s.id == from) {
            if let Some(to_idx) = queue.iter().position(|s| s.id == to) {
                queue.swap(from_idx, to_idx);
            }
        }
        Ok(())
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        let queue = self.queue.lock();
        if let Some((idx, song)) = queue.iter().enumerate().find(|(_, s)| s.id == id) {
            *self.current_index.lock() = idx;
            let mut ipc = self.ipc.lock();
            ipc.send_command(vec!["loadfile", &song.file])?;
        }
        Ok(())
    }

    // ===== Volume Control =====

    fn volume(&mut self) -> Result<u8> {
        let mut ipc = self.ipc.lock();
        let vol: f64 = serde_json::from_value(ipc.get_property("volume")?)?;
        Ok(vol as u8)
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        let mut ipc = self.ipc.lock();

        match volume {
            ValueChange::Absolute(v) => {
                ipc.set_property("volume", serde_json::json!(v as f64))?;
            }
            ValueChange::Increase(delta) => {
                let current: f64 = serde_json::from_value(ipc.get_property("volume")?)?;
                ipc.set_property("volume", serde_json::json!(current + delta as f64))?;
            }
            ValueChange::Decrease(delta) => {
                let current: f64 = serde_json::from_value(ipc.get_property("volume")?)?;
                ipc.set_property("volume", serde_json::json!(current - delta as f64))?;
            }
        }
        Ok(())
    }

    // ===== Playback Options (Stubs - MPV doesn't have these) =====

    fn repeat(&mut self, _repeat: bool) -> Result<()> {
        // MPV doesn't have repeat in the same way
        Ok(())
    }

    fn random(&mut self, _random: bool) -> Result<()> {
        // MPV doesn't have random
        Ok(())
    }

    fn single(&mut self, _single: OnOffOneshot) -> Result<()> {
        Ok(())
    }

    fn consume(&mut self, _consume: OnOffOneshot) -> Result<()> {
        Ok(())
    }

    fn crossfade(&mut self, _seconds: u32) -> Result<()> {
        Ok(())
    }

    // ===== Library Browsing (Stubs - MPV doesn't have a library) =====

    fn lsinfo(&mut self, _path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn list_all(&mut self, _path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn search(&mut self, _filter: &[(Tag, String)]) -> Result<Vec<Song>> {
        Ok(vec![])
    }

    fn find(
        &mut self,
        _filter: &[(Tag, String)],
        _window: Option<(u32, u32)>,
    ) -> Result<Vec<Song>> {
        Ok(vec![])
    }

    fn list_tag(&mut self, _tag: Tag, _filter: Option<&[(Tag, String)]>) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn count(&mut self, _filter: &[(Tag, String)]) -> Result<(usize, std::time::Duration)> {
        Ok((0, std::time::Duration::from_secs(0)))
    }

    // ===== Playlist Management (Stubs) =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        Ok(vec![])
    }

    fn playlist_info_name(&mut self, _name: &str) -> Result<Vec<Song>> {
        Ok(vec![])
    }

    fn load_playlist(&mut self, _name: &str, _position: Option<QueuePosition>) -> Result<()> {
        Ok(())
    }

    fn save_queue_as_playlist(&mut self, _name: &str) -> Result<()> {
        Ok(())
    }

    fn delete_playlist(&mut self, _name: &str) -> Result<()> {
        Ok(())
    }

    fn rename_playlist(&mut self, _old_name: &str, _new_name: &str) -> Result<()> {
        Ok(())
    }

    fn add_to_playlist(&mut self, _playlist: &str, _uri: &str) -> Result<()> {
        Ok(())
    }

    fn delete_from_playlist(&mut self, _playlist: &str, _position: u32) -> Result<()> {
        Ok(())
    }

    fn move_in_playlist(&mut self, _playlist: &str, _from: u32, _to: u32) -> Result<()> {
        Ok(())
    }

    // ===== Sticker Support (Stubs) =====

    fn list_stickers(&mut self, _uri: &str) -> Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }

    fn set_sticker(&mut self, _uri: &str, _key: &str, _value: &str) -> Result<()> {
        Ok(())
    }

    fn delete_sticker(&mut self, _uri: &str, _key: &str) -> Result<()> {
        Ok(())
    }

    // ===== Database Management (Stubs) =====

    fn update(&mut self, _path: Option<&str>) -> Result<u32> {
        Ok(0)
    }

    fn rescan(&mut self, _path: Option<&str>) -> Result<u32> {
        Ok(0)
    }

    // ===== System Info =====

    fn version(&self) -> crate::mpd::version::Version {
        crate::mpd::version::Version { major: 0, minor: 24, patch: 0 }
    }

    fn outputs(&mut self) -> Result<Vec<Output>> {
        Ok(vec![])
    }

    fn decoders(&mut self) -> Result<Vec<Decoder>> {
        Ok(vec![])
    }

    fn partitions(&mut self) -> Result<Vec<String>> {
        Ok(vec![])
    }

    // ===== Backend Identification =====

    fn backend_name(&self) -> &'static str {
        "MPV"
    }

    fn supports_command(&self, _command: &str) -> bool {
        false // MPV doesn't support MPD commands
    }
}
