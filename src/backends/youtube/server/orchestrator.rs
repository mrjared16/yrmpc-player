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
    services::{PlaybackService, QueueService, RepeatMode, PlaybackState, PlaybackStateTracker},
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
    state_tracker: &Arc<PlaybackStateTracker>,
) -> ServerResponse {
    log::info!("play_position called for pos={} (queue len={})", pos, queue.len());

    // Stop current playback first to ensure clean state
    if let Err(e) = playback.stop() {
        log::debug!("stop() returned error (may be idle): {}", e);
    }
    state_tracker.force_set(PlaybackState::Idle);

    // Clear any remaining buffer entries
    if let Err(e) = playback.playlist_clear() {
        log::warn!("Failed to clear MPV buffer: {}", e);
    }

    let queue_len = queue.len();
    if pos >= queue_len {
        return ServerResponse::Error("Position out of bounds".to_string());
    }

    // Update current position in our queue and playback base tracking
    // We do this before URL resolution so that the UI reflects the intent to play
    // even if resolution fails (e.g. network error, or in tests).
    queue.set_current(Some(pos));
    queue.set_playback_base_index(pos);

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

    // Play first track in MPV's playlist
    match playback.playlist_play_index(0) {
        Ok(_) => {
            log::info!("Playing position {} (prefetched {} tracks)", pos, prefetch_count);
            state_tracker.force_set(PlaybackState::Playing);
            ServerResponse::Ok
        }
        Err(e) => {
            log::error!("Failed to play playlist index 0: {}", e);
            state_tracker.force_set(PlaybackState::Idle);
            ServerResponse::Error(e.to_string())
        }
    }
}

/// Internal version of play_position that returns Result
pub fn play_position_internal(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    pos: usize,
    state_tracker: &Arc<PlaybackStateTracker>,
) -> Result<()> {
    match play_position(playback, queue, pos, state_tracker) {
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
    state_tracker: &Arc<PlaybackStateTracker>,
    reason: &str,
) {
    log::info!("Track ended with reason: {}", reason);

    match reason {
        "eof" => handle_eof(playback, queue, state_tracker),
        "error" => handle_playback_error(playback, queue, state_tracker),
        "stop" => {
            log::debug!("Playback stopped by user");
            state_tracker.force_set(PlaybackState::Stopped);
        }
        _ => {
            log::debug!("Unhandled end-file reason: {}", reason);
        }
    }
}

/// Handle natural end of file (eof)
fn handle_eof(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
) {
    log::info!("[DIAG-EOF] handle_eof: start");
    state_tracker.force_set(PlaybackState::EndOfFile);
    let repeat_mode = queue.repeat_mode();
    let current_idx = queue.current_index();
    let queue_len = queue.len();
    log::info!("[DIAG-EOF] handle_eof: repeat_mode={:?} current_idx={:?} queue_len={}", repeat_mode, current_idx, queue_len);

    // Handle Repeat One - replay current track
    if repeat_mode == RepeatMode::One {
        log::info!("[DIAG-EOF] Repeat One: replaying current track");
        if let Some(current_idx) = queue.current_index() {
             let _ = play_position_internal(playback, queue, current_idx, state_tracker);
             return;
        }
        // Fallback to old behavior if no current index (shouldn't happen)
        if let Err(e) = playback.seek(0.0, "absolute") {
            log::error!("Failed to seek to start: {}", e);
        }
        if let Err(e) = playback.unpause() {
            log::error!("Failed to unpause: {}", e);
        }
        state_tracker.force_set(PlaybackState::Playing);
        return;
    }

    // Natural end - MPV auto-advanced to next track
    // Use queue.current_index() as source of truth (NOT mpv_pos) for reliability
    // Check mpv_pos primarily to detect if we advanced within the window or if the window is done
    let mpv_pos = playback.get_playlist_pos().unwrap_or(-1);
    log::info!("[DIAG-EOF] handle_eof: mpv_pos={}", mpv_pos);

    if mpv_pos < 0 {
        log::info!("[DIAG-EOF] handle_eof: calling handle_end_of_window");
        handle_end_of_window(playback, queue, state_tracker, repeat_mode);
    } else {
        log::info!("[DIAG-EOF] handle_eof: calling handle_within_window_advance");
        handle_within_window_advance(playback, queue, state_tracker, mpv_pos as usize);
    }
}

/// Handle when we've reached end of prefetch window
fn handle_end_of_window(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    repeat_mode: RepeatMode,
) {
    log::info!("[DIAG-EOF] handle_end_of_window: start");
    if let Some(current) = queue.current_index() {
        let next_pos = current + 1;
        log::info!("[DIAG-EOF] handle_end_of_window: current={} next_pos={} queue_len={}", current, next_pos, queue.len());
        if next_pos < queue.len() {
            // More songs in queue - start new prefetch window
            log::info!("[DIAG-EOF] End of prefetch window, loading next batch at {}", next_pos);
            let _ = play_position_internal(playback, queue, next_pos, state_tracker);
        } else {
            // At end of queue - check Repeat All
            if repeat_mode == RepeatMode::All {
                log::info!("[DIAG-EOF] Repeat All: looping back to start");
                let _ = play_position_internal(playback, queue, 0, state_tracker);
            } else {
                log::info!("[DIAG-EOF] Reached end of queue, going idle");
                queue.set_current(None);
                state_tracker.force_set(PlaybackState::Idle);
            }
        }
    } else {
        log::warn!("[DIAG-EOF] EOF with no current_index - staying idle");
        state_tracker.force_set(PlaybackState::Idle);
    }
}

/// Handle MPV advancing within the prefetch window
fn handle_within_window_advance(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    mpv_pos: usize,
) {
    let base_index = queue.playback_base_index();
    let new_queue_pos = base_index + mpv_pos;
    log::info!("[DIAG-EOF] handle_within_window_advance: mpv_pos={} base_index={} new_queue_pos={} queue_len={}", mpv_pos, base_index, new_queue_pos, queue.len());

    if new_queue_pos < queue.len() {
        log::info!("[DIAG-EOF] MPV at playlist-pos {}, queue pos now {}", mpv_pos, new_queue_pos);
        queue.set_current(Some(new_queue_pos));
        state_tracker.force_set(PlaybackState::Playing);

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
fn handle_playback_error(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
) {
    log::warn!("Playback error, attempting to skip to next");
    if let Some(current) = queue.current_index() {
        let next_pos = current + 1;
        if next_pos < queue.len() {
            let _ = play_position_internal(playback, queue, next_pos, state_tracker);
        } else {
            state_tracker.force_set(PlaybackState::Idle);
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
pub fn next_track(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
) -> ServerResponse {
    match queue.next_index() {
        Some(idx) => play_position(playback, queue, idx, state_tracker),
        None => ServerResponse::Error("No next track".into()),
    }
}

/// Previous track navigation
pub fn previous_track(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
) -> ServerResponse {
    match queue.previous_index() {
        Some(0) => {
            // At first track, restart current track
            match playback.seek(0.0, "absolute") {
                Ok(_) => {
                    state_tracker.force_set(PlaybackState::Playing);
                    ServerResponse::Ok
                }
                Err(e) => ServerResponse::Error(e.to_string()),
            }
        }
        Some(idx) => play_position(playback, queue, idx, state_tracker),
        None => ServerResponse::Error("No previous track".into()),
    }
}

/// Play by song ID
pub fn play_id(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    id: u32,
    state_tracker: &Arc<PlaybackStateTracker>,
) -> ServerResponse {
    match queue.find_index_by_id(id) {
        Some(pos) => play_position(playback, queue, pos, state_tracker),
        None => ServerResponse::Error("Song not found".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::backends::youtube::config::ExtractorType;
    use crate::domain::Song;

    fn setup_test_services() -> (Arc<PlaybackService>, Arc<QueueService>, Arc<PlaybackStateTracker>) {
        // Mock socket path
        let socket = PathBuf::from("/tmp/test-mpv.sock");

        let playback = Arc::new(PlaybackService::new(&socket, ExtractorType::default()).unwrap());
        let queue = Arc::new(QueueService::new());
        let state_tracker = Arc::new(PlaybackStateTracker::new());

        (playback, queue, state_tracker)
    }

    fn test_song(id: &str) -> Song {
        let mut song = Song::default();
        song.uri = id.to_string();
        song.metadata.insert("title".to_string(), vec![id.to_string()]);
        song
    }

    #[test]
    fn eof_advances_to_next_track() {
        let (playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);
        queue.set_current(Some(0));

        handle_eof(&playback, &queue, &state_tracker);

        assert_eq!(queue.current_index(), Some(1));
    }

    #[test]
    fn eof_with_repeat_one_replays_current() {
        let (playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("Song 1"), None);
        queue.set_current(Some(0));
        queue.set_repeat_mode(RepeatMode::One);

        handle_eof(&playback, &queue, &state_tracker);

        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn eof_at_end_with_repeat_all_loops() {
        let (playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("Song 1"), None);
        queue.add(test_song("Song 2"), None);
        queue.set_current(Some(1));  // Last song
        queue.set_repeat_mode(RepeatMode::All);

        handle_eof(&playback, &queue, &state_tracker);

        assert_eq!(queue.current_index(), Some(0));  // Looped
    }

    #[test]
    fn eof_at_end_without_repeat_goes_idle() {
        let (playback, queue, state_tracker) = setup_test_services();
        queue.add(test_song("Song 1"), None);
        queue.set_current(Some(0));
        queue.set_repeat_mode(RepeatMode::Off);

        handle_eof(&playback, &queue, &state_tracker);

        assert_eq!(queue.current_index(), None);
        assert_eq!(state_tracker.get(), PlaybackState::Idle);
    }
}
