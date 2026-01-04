//! Playback control handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{protocol::ServerResponse, services::PlaybackService};

/// Handle Play command
pub fn handle_play(playback: &Arc<PlaybackService>, event_tx: &Sender<String>) -> ServerResponse {
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
