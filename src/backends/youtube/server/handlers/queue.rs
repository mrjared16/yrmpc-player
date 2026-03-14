//! Queue management handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;
use parking_lot::Mutex;
use tokio::runtime::{Builder, Handle};

use super::{super::orchestrator::PREFETCH_WINDOW_SIZE, queue_events::QueueEventHandler};
use crate::{
    backends::youtube::{
        media::{MediaPreparer, PreloadTier, PreparedMedia},
        protocol::{ServerResponse, SongData},
        services::{PlaybackService, QueueService},
    },
    domain::Song,
    shared::play_queue::{PlayQueue, QueueCommand},
};

/// Handle Add command (URI only, legacy)
pub fn handle_add(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    uri: &str,
    position: Option<u32>,
    play_queue: &Arc<Mutex<PlayQueue>>,
    queue_event_handler: &Mutex<QueueEventHandler>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    let mut song = Song::default();
    song.uri = uri.to_string();
    song.metadata.insert("title".into(), vec![uri.to_string()]);

    let song_data = SongData::from(song);
    handle_add_song(
        queue,
        playback,
        event_tx,
        song_data,
        position,
        play_queue,
        queue_event_handler,
        media_preparer,
    )
}

/// Handle AddSong command with full metadata
pub fn handle_add_song(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    song_data: SongData,
    position: Option<u32>,
    play_queue: &Arc<Mutex<PlayQueue>>,
    queue_event_handler: &Mutex<QueueEventHandler>,
    media_preparer: &Arc<dyn MediaPreparer>,
) -> ServerResponse {
    let video_id = song_data.file.clone();

    // Get rolling window bounds BEFORE add
    let base = queue.playback_base_index();
    let queue_len = queue.len();

    // 1. Store metadata in QueueService immediately
    let song = song_data.to_song();
    let queue_id = queue.add(song.clone(), position);
    log::debug!("Added song to queue: id={}, video_id={}", queue_id, video_id);

    // 2. Route through PlayQueue for event-driven updates
    let events = play_queue.lock().apply(QueueCommand::Add { song });
    for event in events {
        queue_event_handler.lock().handle(event);
    }

    // 3. Calculate where the song was inserted
    let insert_pos = position.map(|p| (p as usize).min(queue_len)).unwrap_or(queue_len);

    // 4. If inserted within the rolling window AND we're currently playing, resolve
    //    URL and add to MPV buffer for seamless playback
    if queue.current_index().is_some()
        && insert_pos >= base
        && insert_pos < base + PREFETCH_WINDOW_SIZE
    {
        let window_offset = insert_pos.saturating_sub(base);
        let tier = match window_offset {
            0 => PreloadTier::Immediate,
            1 => PreloadTier::Gapless,
            _ => PreloadTier::Eager,
        };

        match prepare_media_blocking(media_preparer, &video_id, tier)
            .and_then(|prepared| playback.build_runtime_input(&video_id, &prepared))
        {
            Ok(input) => {
                if let Err(e) = playback.playlist_append_input(&input) {
                    log::warn!("Failed to append to MPV buffer: {}", e);
                } else {
                    // Move from end to correct position in MPV buffer
                    let mpv_insert_pos = insert_pos.saturating_sub(base);
                    let mpv_current_end = playback.get_playlist_count().unwrap_or(1);
                    if mpv_current_end > 1 && mpv_insert_pos < mpv_current_end - 1 {
                        if let Err(e) = playback.playlist_move(mpv_current_end - 1, mpv_insert_pos)
                        {
                            log::warn!("Failed to reorder MPV buffer: {}", e);
                        }
                    }
                    log::debug!("Added song to MPV buffer at position {}", mpv_insert_pos);
                }
            }
            Err(e) => {
                log::warn!("Failed to prepare media for window insert: {}", e);
            }
        }
    } else if queue.current_index().is_some() {
        if let Some(track_id) = extract_video_id(&video_id) {
            media_preparer.prefetch(&track_id, PreloadTier::Background);
            log::debug!("Triggered background media prefetch for {}", track_id);
        }
    }

    // 5. Notify clients
    let _ = event_tx.send("playlist".to_string());
    ServerResponse::Ok
}

fn extract_video_id(uri: &str) -> Option<String> {
    if let Some(id) = uri.strip_prefix("youtube://") {
        return Some(id.to_string());
    }

    if !uri.is_empty() && !uri.contains("://") {
        return Some(uri.to_string());
    }

    None
}

fn prepare_media_blocking(
    media_preparer: &Arc<dyn MediaPreparer>,
    track_id: &str,
    tier: PreloadTier,
) -> anyhow::Result<PreparedMedia> {
    if let Ok(handle) = Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(media_preparer.prepare(track_id, tier)))
    } else {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(media_preparer.prepare(track_id, tier))
    }
}

/// Handle DeleteId command
pub fn handle_delete_id(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    id: u32,
    play_queue: &Arc<Mutex<PlayQueue>>,
    queue_event_handler: &Mutex<QueueEventHandler>,
) -> ServerResponse {
    let current_idx = queue.current_index();
    let deleting_current = current_idx
        .and_then(|idx| queue.get_by_index(idx).ok())
        .map(|s| s.id == Some(id))
        .unwrap_or(false);

    let base = queue.playback_base_index();

    match queue.remove(id) {
        Ok((_removed_item, removed_pos)) => {
            let events = play_queue.lock().apply(QueueCommand::Remove { id: id as u64 });
            for event in events {
                queue_event_handler.lock().handle(event);
            }

            if removed_pos >= base && removed_pos < base + PREFETCH_WINDOW_SIZE {
                let mpv_idx = removed_pos - base;
                if let Err(e) = playback.playlist_remove(mpv_idx) {
                    log::warn!("Failed to sync MPV buffer on delete: {}", e);
                }
                log::debug!("Removed song from MPV buffer at index {}", mpv_idx);
            }

            if deleting_current {
                if let Err(e) = playback.stop() {
                    log::warn!("Failed to stop playback after delete: {}", e);
                }
                let _ = event_tx.send("player".to_string());
            }
            let _ = event_tx.send("playlist".to_string());
            ServerResponse::Ok
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle Clear command
pub fn handle_clear(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    play_queue: &Arc<Mutex<PlayQueue>>,
    queue_event_handler: &Mutex<QueueEventHandler>,
) -> ServerResponse {
    queue.clear();

    let events = play_queue.lock().apply(QueueCommand::Clear);
    for event in events {
        queue_event_handler.lock().handle(event);
    }

    if let Err(e) = playback.stop() {
        log::warn!("Failed to stop playback on clear: {}", e);
    }
    let _ = event_tx.send("playlist".to_string());
    let _ = event_tx.send("player".to_string());
    ServerResponse::Ok
}

/// Handle MoveId command
pub fn handle_move_id(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    from: u32,
    to: u32,
    play_queue: &Arc<Mutex<PlayQueue>>,
    queue_event_handler: &Mutex<QueueEventHandler>,
) -> ServerResponse {
    let base = queue.playback_base_index();

    match queue.move_song(from, to) {
        Ok((from_idx, to_idx)) => {
            let events = play_queue
                .lock()
                .apply(QueueCommand::Move { id: from as u64, to_position: to_idx });
            for event in events {
                queue_event_handler.lock().handle(event);
            }

            let from_in_window = from_idx >= base && from_idx < base + PREFETCH_WINDOW_SIZE;
            let to_in_window = to_idx >= base && to_idx < base + PREFETCH_WINDOW_SIZE;

            if from_in_window || to_in_window {
                let from_mpv = from_idx.saturating_sub(base);
                let to_mpv = to_idx.saturating_sub(base);

                if from_mpv < PREFETCH_WINDOW_SIZE && to_mpv < PREFETCH_WINDOW_SIZE {
                    if let Err(e) = playback.playlist_move(from_mpv, to_mpv) {
                        log::warn!("Failed to sync MPV buffer on move: {}", e);
                    }
                    log::debug!("Moved song in MPV buffer: {} -> {}", from_mpv, to_mpv);
                }
            }

            let _ = event_tx.send("playlist".to_string());
            ServerResponse::Ok
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}
