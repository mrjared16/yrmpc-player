//! Status query handlers.

use std::sync::Arc;

use crate::backends::youtube::{
    protocol::{ServerResponse, SongData, StatusData},
    server::queue_view::QueueView,
    services::PlaybackService,
};

/// Handle GetStatus command
pub fn handle_get_status(playback: &Arc<PlaybackService>, queue: &dyn QueueView) -> ServerResponse {
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
        let song_id = queue.get_by_index(pos).and_then(|s| s.id);
        let state = if paused { "pause" } else { "play" };
        (state.to_string(), Some(pos as u32), song_id)
    };

    // Get repeat/shuffle state
    let repeat = queue.repeat_label();
    let shuffle = queue.shuffle_enabled();
    let next_queue_pos = queue.next_index().map(|idx| idx as u32);

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
pub fn handle_get_current_song(_playback: &Arc<PlaybackService>, queue: &dyn QueueView) -> ServerResponse {
    let queue_pos = match queue.current_index() {
        Some(pos) => pos,
        None => return ServerResponse::Song(None),
    };

    match queue.get_by_index(queue_pos) {
        Some(song) => ServerResponse::Song(Some(SongData::from(song))),
        None => ServerResponse::Song(None),
    }
}

/// Handle GetPlaylist command
pub fn handle_get_playlist(queue: &dyn QueueView) -> ServerResponse {
    let songs = queue.get_all();
    ServerResponse::Playlist(songs.into_iter().map(SongData::from).collect())
}
