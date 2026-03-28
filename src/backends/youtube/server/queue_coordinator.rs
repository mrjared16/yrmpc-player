use std::sync::Arc;

use crossbeam::channel::Sender;
use parking_lot::Mutex;

use super::{
    handlers::{queue_events::QueueEventHandler, stable_track_id},
    orchestrator::{Orchestrator, PREFETCH_WINDOW_SIZE},
};
use crate::{
    backends::youtube::{
        protocol::{ServerResponse, SongData},
        services::{PlaybackService, QueueService},
    },
    domain::Song,
    shared::play_queue::{PlayQueue, QueueCommand, QueueEvent},
};

pub struct QueueCoordinator {
    queue: Arc<QueueService>,
    playback: Arc<PlaybackService>,
    play_queue: Arc<Mutex<PlayQueue>>,
    queue_event_handler: Mutex<QueueEventHandler>,
    orchestrator: Arc<Orchestrator>,
    event_tx: Sender<String>,
}

impl QueueCoordinator {
    pub fn new(
        queue: Arc<QueueService>,
        playback: Arc<PlaybackService>,
        play_queue: Arc<Mutex<PlayQueue>>,
        queue_event_handler: QueueEventHandler,
        orchestrator: Arc<Orchestrator>,
        event_tx: Sender<String>,
    ) -> Self {
        Self {
            queue,
            playback,
            play_queue,
            queue_event_handler: Mutex::new(queue_event_handler),
            orchestrator,
            event_tx,
        }
    }

    pub fn apply(&self, cmd: QueueCommand) -> Vec<QueueEvent> {
        let events = self.play_queue.lock().apply(cmd);
        let mut handler = self.queue_event_handler.lock();
        for event in &events {
            handler.handle(event.clone());
        }
        events
    }

    pub fn add_uri(&self, uri: &str, position: Option<u32>) -> ServerResponse {
        let mut song = Song::default();
        song.uri = uri.to_string();
        song.metadata.insert("title".into(), vec![uri.to_string()]);
        self.add_song(SongData::from(song), position)
    }

    pub fn add_song(&self, song_data: SongData, position: Option<u32>) -> ServerResponse {
        let video_id = stable_track_id(&song_data.file);
        let had_active_playback = self.queue.current_index().is_some();
        let base = self.queue.playback_base_index();
        let queue_len = self.queue.len();

        let song = song_data.to_song();
        let queue_id = self.queue.add(song.clone(), position);
        log::debug!("Added song to queue: id={}, video_id={}", queue_id, video_id);

        let cmd = match position {
            Some(pos) => QueueCommand::AddAt { song, position: (pos as usize).min(queue_len) },
            None => QueueCommand::Add { song },
        };
        self.apply(cmd);

        let insert_pos = position.map(|p| (p as usize).min(queue_len)).unwrap_or(queue_len);
        if had_active_playback && insert_pos < base + PREFETCH_WINDOW_SIZE {
            if let Err(e) = self.orchestrator.reconcile_active_window_after_queue_mutation() {
                log::warn!("Failed to reconcile active playback window after add: {e}");
            }
        }

        let _ = self.event_tx.send("playlist".to_string());
        ServerResponse::Ok
    }

    pub fn delete_id(&self, id: u32) -> ServerResponse {
        let current_idx = self.queue.current_index();
        let had_active_playback = current_idx.is_some();
        let deleting_current = current_idx
            .and_then(|idx| self.queue.get_by_index(idx).ok())
            .map(|s| s.id == Some(id))
            .unwrap_or(false);
        let base = self.queue.playback_base_index();

        match self.queue.remove(id) {
            Ok((_removed_item, removed_pos)) => {
                self.apply(QueueCommand::Remove { id: id as u64 });
                if deleting_current {
                    if let Err(e) = self.playback.stop() {
                        log::warn!("Failed to stop playback after delete: {e}");
                    }
                    let _ = self.event_tx.send("player".to_string());
                } else if had_active_playback && removed_pos < base + PREFETCH_WINDOW_SIZE {
                    if let Err(e) = self.orchestrator.reconcile_active_window_after_queue_mutation()
                    {
                        log::warn!("Failed to reconcile active playback window after delete: {e}");
                    }
                }
                let _ = self.event_tx.send("playlist".to_string());
                ServerResponse::Ok
            }
            Err(e) => ServerResponse::Error(e.to_string()),
        }
    }

    pub fn clear(&self) -> ServerResponse {
        self.queue.clear();
        self.apply(QueueCommand::Clear);

        if let Err(e) = self.playback.stop() {
            log::warn!("Failed to stop playback on clear: {e}");
        }
        let _ = self.event_tx.send("playlist".to_string());
        let _ = self.event_tx.send("player".to_string());
        ServerResponse::Ok
    }

    pub fn move_id(&self, from: u32, to: u32) -> ServerResponse {
        let base = self.queue.playback_base_index();
        let had_active_playback = self.queue.current_index().is_some();

        match self.queue.move_song(from, to) {
            Ok((from_idx, to_idx)) => {
                self.apply(QueueCommand::Move { id: from as u64, to_position: to_idx });

                if had_active_playback
                    && (from_idx < base + PREFETCH_WINDOW_SIZE
                        || to_idx < base + PREFETCH_WINDOW_SIZE)
                {
                    if let Err(e) = self.orchestrator.reconcile_active_window_after_queue_mutation()
                    {
                        log::warn!("Failed to reconcile active playback window after move: {e}");
                    }
                }

                let _ = self.event_tx.send("playlist".to_string());
                ServerResponse::Ok
            }
            Err(e) => ServerResponse::Error(e.to_string()),
        }
    }

    pub fn queue(&self) -> &Arc<QueueService> {
        &self.queue
    }
}
