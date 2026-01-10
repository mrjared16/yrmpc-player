//! Options and volume handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    protocol::ServerResponse,
    server::orchestrator::PREFETCH_WINDOW_SIZE,
    services::{PlaybackService, QueueService, RepeatMode},
};

/// Handle GetVolume command
pub fn handle_get_volume(playback: &Arc<PlaybackService>) -> ServerResponse {
    match playback.get_volume() {
        Ok(vol) => ServerResponse::Volume(vol),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle SetVolume command
pub fn handle_set_volume(playback: &Arc<PlaybackService>, vol: u8) -> ServerResponse {
    match playback.set_volume(vol) {
        Ok(_) => ServerResponse::Ok,
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle AdjustVolume command
pub fn handle_adjust_volume(playback: &Arc<PlaybackService>, delta: i8) -> ServerResponse {
    match playback.adjust_volume(delta) {
        Ok(_) => ServerResponse::Ok,
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle SetRepeat command
pub fn handle_set_repeat(
    queue: &Arc<QueueService>,
    event_tx: &Sender<String>,
    mode: &str,
) -> ServerResponse {
    let repeat_mode = match mode {
        "one" => RepeatMode::One,
        "all" => RepeatMode::All,
        _ => RepeatMode::Off,
    };
    queue.set_repeat_mode(repeat_mode);
    let _ = event_tx.send("options".to_string());
    ServerResponse::Ok
}

/// Handle SetShuffle command
pub fn handle_set_shuffle(
    queue: &Arc<QueueService>,
    event_tx: &Sender<String>,
    enabled: bool,
) -> ServerResponse {
    queue.set_shuffle_enabled(enabled);

    if let Some(current_pos) = queue.current_index() {
        queue.build_prefetch_window(current_pos, PREFETCH_WINDOW_SIZE);
    }

    let _ = event_tx.send("options".to_string());
    ServerResponse::Ok
}
