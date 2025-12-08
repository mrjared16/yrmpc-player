//! Orchestrator for YouTube backend server.
//!
//! Contains the complex business logic for:
//! - Prefetch window management (3-track rolling window)
//! - Track-ended handling (auto-advance, repeat modes)
//! - Play position logic with MPRIS updates
//!
//! This module is used by both the main server and the internal event processor.

use std::sync::Arc;

use anyhow::Result;

use crate::backends::youtube::{
    protocol::{ServerResponse, SongData},
    services::{PlaybackService, QueueService, RepeatMode},
};

/// Prefetch window size for gapless playback
pub const PREFETCH_WINDOW_SIZE: usize = 3;

/// Play a track at the given queue position.
///
/// This rebuilds MPV's buffer with current + next tracks,
/// enabling gapless auto-advance when a song ends.
///
/// Uses EDL URLs when audio is cached for instant playback.
pub fn play_position(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
) -> ServerResponse {
    log::info!("play_position called for pos={} (queue len={})", pos, queue.len());

    // Stop current playback first to ensure clean state
    if let Err(e) = playback.stop() {
        log::debug!("stop() returned error (may be idle): {}", e);
    }

    // Clear any remaining buffer entries
    if let Err(e) = playback.playlist_clear() {
        log::warn!("Failed to clear MPV buffer: {}", e);
    }

    let queue_len = queue.len();
    if pos >= queue_len {
        return ServerResponse::Error("Position out of bounds".to_string());
    }

    // Resolve and append current + next tracks (rolling window)
    let prefetch_count = PREFETCH_WINDOW_SIZE.min(queue_len - pos);
    let mut first_url_metadata: Option<(String, String)> = None;

    for i in 0..prefetch_count {
        let idx = pos + i;
        match queue.get_by_index(idx) {
            Ok(song) => {
                let video_id = &song.uri;

                match playback.build_playback_url(video_id) {
                    Ok(url) => {
                        if let Err(e) = playback.playlist_append(&url) {
                            log::error!("Failed to append to playlist: {}", e);
                        } else {
                            let is_edl = url.starts_with("edl://");
                            log::debug!(
                                "Prefetched track {} (queue pos {}){}",
                                i, idx,
                                if is_edl { " [EDL: instant]" } else { "" }
                            );

                            // Save metadata for MPRIS (first track only)
                            if i == 0 {
                                let title = song.metadata.get("title")
                                    .and_then(|v| v.first())
                                    .cloned()
                                    .unwrap_or_else(|| video_id.clone());
                                let artist = song.metadata.get("artist")
                                    .and_then(|v| v.first())
                                    .cloned()
                                    .unwrap_or_default();
                                first_url_metadata = Some((title, artist));
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to resolve stream URL for {}: {}", video_id, e);
                        if i == 0 {
                            return ServerResponse::Error(format!("Failed to resolve stream: {}", e));
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to get song at index {}: {}", idx, e);
            }
        }
    }

    // Set MPRIS metadata for first track
    if let Some((title, artist)) = first_url_metadata {
        if let Err(e) = playback.set_media_title(&title, &artist) {
            log::warn!("Failed to set media title: {}", e);
        }
    }

    // Update current position in our queue and playback base tracking
    queue.set_current(Some(pos));
    queue.set_playback_base_index(pos);

    // Play first track in MPV's playlist
    match playback.playlist_play_index(0) {
        Ok(_) => {
            log::info!("Playing position {} (prefetched {} tracks)", pos, prefetch_count);
            ServerResponse::Ok
        }
        Err(e) => {
            log::error!("Failed to play playlist index 0: {}", e);
            ServerResponse::Error(e.to_string())
        }
    }
}

/// Internal version of play_position that returns Result
pub fn play_position_internal(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
) -> Result<()> {
    match play_position(playback, queue, pos) {
        ServerResponse::Ok => Ok(()),
        ServerResponse::Error(e) => Err(anyhow::anyhow!("{}", e)),
        _ => Ok(()),
    }
}

/// Handle track ended event from MPV.
///
/// When MPV auto-advances (end-file:eof), we need to:
/// 1. Sync our queue position with MPV's playlist-pos
/// 2. Append next track to maintain prefetch window
/// 3. Update MPRIS metadata for new track
/// 4. Handle repeat modes
pub fn handle_track_ended(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    reason: &str,
) {
    log::info!("Track ended with reason: {}", reason);

    match reason {
        "eof" => handle_eof(playback, queue),
        "error" => handle_playback_error(playback, queue),
        "stop" => {
            log::debug!("Playback stopped by user");
        }
        _ => {
            log::debug!("Unhandled end-file reason: {}", reason);
        }
    }
}

/// Handle natural end of file (eof)
fn handle_eof(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) {
    let repeat_mode = queue.repeat_mode();

    // Handle Repeat One - replay current track
    if repeat_mode == RepeatMode::One {
        log::info!("Repeat One: replaying current track");
        if let Err(e) = playback.seek(0.0, "absolute") {
            log::error!("Failed to seek to start: {}", e);
        }
        if let Err(e) = playback.unpause() {
            log::error!("Failed to unpause: {}", e);
        }
        return;
    }

    // Natural end - MPV auto-advanced to next track
    let mpv_pos = playback.get_playlist_pos().unwrap_or(-1);

    if mpv_pos < 0 {
        handle_end_of_window(playback, queue, repeat_mode);
    } else {
        handle_within_window_advance(playback, queue, mpv_pos as usize);
    }
}

/// Handle when we've reached end of prefetch window
fn handle_end_of_window(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    repeat_mode: RepeatMode,
) {
    if let Some(current) = queue.current_index() {
        let next_pos = current + 1;
        if next_pos < queue.len() {
            // More songs in queue - start new prefetch window
            log::info!("End of prefetch window, loading next batch at {}", next_pos);
            let _ = play_position_internal(playback, queue, next_pos);
        } else {
            // At end of queue - check Repeat All
            if repeat_mode == RepeatMode::All {
                log::info!("Repeat All: looping back to start");
                let _ = play_position_internal(playback, queue, 0);
            } else {
                log::info!("Reached end of queue");
                queue.set_current(None);
            }
        }
    }
}

/// Handle MPV advancing within the prefetch window
fn handle_within_window_advance(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    mpv_pos: usize,
) {
    let base_index = queue.playback_base_index();
    let new_queue_pos = base_index + mpv_pos;

    if new_queue_pos < queue.len() {
        log::debug!("MPV at playlist-pos {}, queue pos now {}", mpv_pos, new_queue_pos);
        queue.set_current(Some(new_queue_pos));

        // Update MPRIS metadata for current track
        if let Ok(song) = queue.get_by_index(new_queue_pos) {
            let title = song.metadata.get("title")
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_else(|| song.uri.clone());
            let artist = song.metadata.get("artist")
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_default();
            let _ = playback.set_media_title(&title, &artist);
        }

        // Maintain prefetch window - append next track if available
        let prefetch_pos = new_queue_pos + PREFETCH_WINDOW_SIZE;
        if prefetch_pos < queue.len() {
            if let Ok(song) = queue.get_by_index(prefetch_pos) {
                if let Ok(url) = playback.build_playback_url(&song.uri) {
                    let _ = playback.playlist_append(&url);
                    log::debug!("Extended prefetch window to queue pos {}", prefetch_pos);
                }
            }
        }
    }
}

/// Handle playback error by skipping to next track
fn handle_playback_error(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) {
    log::warn!("Playback error, attempting to skip to next");
    if let Some(current) = queue.current_index() {
        let next_pos = current + 1;
        if next_pos < queue.len() {
            let _ = play_position_internal(playback, queue, next_pos);
        }
    }
}

/// Prefetch upcoming tracks in background
pub fn prefetch_upcoming(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) {
    if let Some(current) = queue.current_index() {
        let all_songs = queue.get_all();
        let video_ids: Vec<String> = all_songs
            .iter()
            .skip(current + 1)
            .take(5)
            .map(|s| s.uri.clone())
            .collect();

        if !video_ids.is_empty() {
            playback.prefetch(video_ids);
        }
    }
}

/// Get the current song based on MPV's position
pub fn get_current_song(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) -> ServerResponse {
    let mpv_playlist_pos = playback.get_playlist_pos().unwrap_or(-1);
    if mpv_playlist_pos < 0 {
        return ServerResponse::Song(None);
    }

    let queue_pos = queue.playback_base_index() + (mpv_playlist_pos as usize);

    match queue.get_by_index(queue_pos) {
        Ok(song) => ServerResponse::Song(Some(SongData::from(song))),
        Err(_) => ServerResponse::Song(None),
    }
}

/// Get playlist as SongData list
pub fn get_playlist(queue: &Arc<QueueService>) -> ServerResponse {
    let songs = queue.get_all();
    ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
}

/// Next track navigation
pub fn next_track(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) -> ServerResponse {
    match queue.next_index() {
        Some(idx) => play_position(playback, queue, idx),
        None => ServerResponse::Error("No next track".into()),
    }
}

/// Previous track navigation
pub fn previous_track(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>) -> ServerResponse {
    match queue.previous_index() {
        Some(0) => {
            // At first track, restart current track
            match playback.seek(0.0, "absolute") {
                Ok(_) => ServerResponse::Ok,
                Err(e) => ServerResponse::Error(e.to_string()),
            }
        }
        Some(idx) => play_position(playback, queue, idx),
        None => ServerResponse::Error("No previous track".into()),
    }
}

/// Play by song ID
pub fn play_id(playback: &Arc<PlaybackService>, queue: &Arc<QueueService>, id: u32) -> ServerResponse {
    match queue.find_index_by_id(id) {
        Some(pos) => play_position(playback, queue, pos),
        None => ServerResponse::Error("Song not found".into()),
    }
}
