//! Status query handlers.

use std::sync::Arc;

use crate::backends::youtube::{
    protocol::{ServerResponse, SongData, StatusData},
    services::{PlaybackService, QueueService, RepeatMode},
};

/// Handle GetStatus command
pub fn handle_get_status(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
) -> ServerResponse {
    let paused = playback.is_paused().unwrap_or(true);
    let volume = playback.get_volume().unwrap_or(100);
    let time_pos = playback.get_position().unwrap_or(0.0);
    let duration = playback.get_duration().unwrap_or(0.0);

    let queue_len = queue.len();
    let current_queue_idx = queue.current_index();

    // Determine playback state
    let (state, current_pos, current_id) = if queue_len == 0 || current_queue_idx.is_none() {
        ("stop".to_string(), None, None)
    } else {
        let pos = current_queue_idx.unwrap();
        let song_id = queue.get_by_index(pos).ok().and_then(|s| s.id);
        let state = if paused { "pause" } else { "play" };
        (state.to_string(), Some(pos as u32), song_id)
    };

    // Get repeat/shuffle state
    let repeat = match queue.repeat_mode() {
        RepeatMode::Off => "off",
        RepeatMode::One => "one",
        RepeatMode::All => "all",
    };
    let shuffle = queue.shuffle_enabled();

    let next_queue_pos = if shuffle {
        queue.get_prefetched_at(1).map(|idx| idx as u32)
    } else {
        current_queue_idx
            .and_then(|idx| if idx + 1 < queue_len { Some((idx + 1) as u32) } else { None })
    };

    ServerResponse::Status(StatusData {
        state,
        volume,
        elapsed_ms: Some((time_pos * 1000.0) as u64),
        duration_ms: Some((duration * 1000.0) as u64),
        playlist_length: queue_len as u32,
        current_pos,
        current_id,
        repeat: repeat.to_string(),
        shuffle,
        next_queue_pos,
    })
}

/// Handle GetCurrentSong command
pub fn handle_get_current_song(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
) -> ServerResponse {
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

/// Handle GetPlaylist command
pub fn handle_get_playlist(queue: &Arc<QueueService>) -> ServerResponse {
    let songs = queue.get_all();
    ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
}
