//! Playback control handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    protocol::ServerResponse,
    server::orchestrator,
    services::{PlaybackService, PlaybackState, PlaybackStateTracker, QueueService},
};

/// Handle Play command
///
/// When in Idle state with items in queue, reloads the track at current
/// position. Otherwise just unpauses playback.
pub fn handle_play(
    playback: &Arc<PlaybackService>,
    queue: &Arc<QueueService>,
    state_tracker: &Arc<PlaybackStateTracker>,
    event_tx: &Sender<String>,
) -> ServerResponse {
    let state = state_tracker.get();

    if matches!(state, PlaybackState::Idle | PlaybackState::Stopped) && queue.len() > 0 {
        let pos = queue.current_index().unwrap_or(0);
        log::info!("[STATE] play state={:?} pos={}", state, pos);
        return orchestrator::play_position_sync(playback, queue, pos, state_tracker);
    }

    log::info!("[STATE] unpause");
    match playback.unpause() {
        Ok(_) => {
            let _ = event_tx.send("player".to_string());
            ServerResponse::Ok
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle Pause command
pub fn handle_pause(playback: &Arc<PlaybackService>, event_tx: &Sender<String>) -> ServerResponse {
    log::info!("[STATE] pause");
    match playback.pause() {
        Ok(_) => {
            let _ = event_tx.send("player".to_string());
            ServerResponse::Ok
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle Stop command
pub fn handle_stop(playback: &Arc<PlaybackService>, event_tx: &Sender<String>) -> ServerResponse {
    log::info!("[STATE] stop");
    match playback.stop() {
        Ok(_) => {
            let _ = event_tx.send("player".to_string());
            ServerResponse::Ok
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle SeekAbsolute command
pub fn handle_seek_absolute(playback: &Arc<PlaybackService>, pos: f64) -> ServerResponse {
    match playback.seek(pos, "absolute") {
        Ok(_) => ServerResponse::Ok,
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle SeekRelative command
pub fn handle_seek_relative(playback: &Arc<PlaybackService>, delta: f64) -> ServerResponse {
    match playback.seek(delta, "relative") {
        Ok(_) => ServerResponse::Ok,
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}
