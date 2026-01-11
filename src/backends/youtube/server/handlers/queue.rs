//! Queue management handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;

use super::super::orchestrator::PREFETCH_WINDOW_SIZE;
use crate::{
    backends::youtube::{
        protocol::{ServerResponse, SongData},
        services::{PlaybackService, QueueService},
    },
    domain::Song,
};

/// Handle Add command (URI only, legacy)
pub fn handle_add(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    uri: &str,
    position: Option<u32>,
) -> ServerResponse {
    // Create simple song from URI (legacy - prefer handle_add_song)
    let mut song = Song::default();
    song.uri = uri.to_string();
    song.metadata.insert("title".into(), vec![uri.to_string()]);

    let song_data = SongData::from(song);
    handle_add_song(queue, playback, event_tx, song_data, position)
}

/// Handle AddSong command with full metadata
pub fn handle_add_song(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    song_data: SongData,
    position: Option<u32>,
) -> ServerResponse {
    let video_id = song_data.file.clone();

    // Get rolling window bounds BEFORE add
    let base = queue.playback_base_index();
    let queue_len = queue.len();

    // 1. Store metadata in QueueService immediately
    let song = song_data.to_song();
    let queue_id = queue.add(song, position);
    log::debug!("Added song to queue: id={}, video_id={}", queue_id, video_id);

    // 2. Calculate where the song was inserted
    let insert_pos = position.map(|p| (p as usize).min(queue_len)).unwrap_or(queue_len);

    // 3. If inserted within the rolling window AND we're currently playing, resolve
    //    URL and add to MPV buffer for seamless playback
    if queue.current_index().is_some()
        && insert_pos >= base
        && insert_pos < base + PREFETCH_WINDOW_SIZE
    {
        match playback.build_playback_url(&video_id) {
            Ok(url) => {
                if let Err(e) = playback.playlist_append(&url) {
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
                log::warn!("Failed to resolve URL for window insert: {}", e);
            }
        }
    } else if queue.current_index().is_some() {
        playback.prefetch_audio_batch(vec![video_id.clone()]);
        log::debug!("Triggered background audio prefetch for {}", video_id);
    }

    // 5. Notify clients
    let _ = event_tx.send("playlist".to_string());
    ServerResponse::Ok
}

/// Handle DeleteId command
pub fn handle_delete_id(
    queue: &Arc<QueueService>,
    playback: &Arc<PlaybackService>,
    event_tx: &Sender<String>,
    id: u32,
) -> ServerResponse {
    // Check if we're deleting the currently playing track
    let current_idx = queue.current_index();
    let deleting_current = current_idx
        .and_then(|idx| queue.get_by_index(idx).ok())
        .map(|s| s.id == Some(id))
        .unwrap_or(false);

    // Get the rolling window bounds BEFORE removal
    let base = queue.playback_base_index();

    match queue.remove(id) {
        Ok((_removed_item, removed_pos)) => {
            // Sync MPV buffer if deleted song was in the rolling window
            if removed_pos >= base && removed_pos < base + PREFETCH_WINDOW_SIZE {
                let mpv_idx = removed_pos - base;
                if let Err(e) = playback.playlist_remove(mpv_idx) {
                    log::warn!("Failed to sync MPV buffer on delete: {}", e);
                }
                log::debug!("Removed song from MPV buffer at index {}", mpv_idx);
            }

            // If we deleted the playing track, stop playback
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
) -> ServerResponse {
    queue.clear();
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
) -> ServerResponse {
    let base = queue.playback_base_index();

    match queue.move_song(from, to) {
        Ok((from_idx, to_idx)) => {
            // Sync MPV buffer if either position affects the rolling window
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
