//! Playback control handlers.

use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::backends::youtube::{
    protocol::ServerResponse,
    server::orchestrator::Orchestrator,
    services::{PlaybackService, PlaybackState},
};

/// Handle Play command
///
/// When in Idle state with items in queue, reloads the track at current
/// position. Otherwise just unpauses playback.
pub fn handle_play(
    orchestrator: &Orchestrator,
    event_tx: &Sender<String>,
) -> ServerResponse {
    let state = orchestrator.state_tracker().get();

    if matches!(state, PlaybackState::Idle | PlaybackState::Stopped) && orchestrator.queue().len() > 0 {
        let pos = orchestrator.queue().current_index().unwrap_or(0);
        log::info!("[STATE] play state={:?} pos={}", state, pos);
        return orchestrator.play_position_sync(pos);
    }

    log::info!("[STATE] unpause");
    match orchestrator.playback().unpause() {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::*;
    use crate::{
        backends::youtube::{
            audio::AudioDeliveryPlanner,
            config::{AudioDeliveryMode, ExtractorType},
            media::{MediaPreparer, PreloadTier},
            server::test_support::{MpvTestGuard, RecordingMediaPreparer, acquire_mpv_test_guard},
            services::{PlaybackStateTracker, QueueService},
            url_resolver::UrlResolver,
        },
        domain::Song,
    };

    fn test_song(uri: &str) -> Song {
        let mut song = Song::default();
        song.uri = uri.to_string();
        song.metadata.insert("title".to_string(), vec![uri.to_string()]);
        song
    }

    fn setup_orchestrator() -> (MpvTestGuard, TempDir, Orchestrator, Arc<RecordingMediaPreparer>) {
        let mpv_guard = acquire_mpv_test_guard();
        let temp_dir = TempDir::new().unwrap();
        let socket = temp_dir.path().join("test-mpv.sock");
        let url_resolver = Arc::new(UrlResolver::new(ExtractorType::default()));
        let playback = Arc::new(
            PlaybackService::new(
                &socket,
                url_resolver,
                None,
                AudioDeliveryPlanner.plan(AudioDeliveryMode::Direct),
                None,
            )
            .unwrap(),
        );
        let queue = Arc::new(QueueService::new());
        let state_tracker = Arc::new(PlaybackStateTracker::new());
        let recording = Arc::new(RecordingMediaPreparer::default());
        let media_preparer: Arc<dyn MediaPreparer> = recording.clone();
        let orchestrator = Orchestrator::new(playback, queue, state_tracker, media_preparer);

        (mpv_guard, temp_dir, orchestrator, recording)
    }

    #[test]
    fn play_from_idle_reuses_authoritative_play_position_path() {
        let (_mpv_guard, _temp_dir, orchestrator, recording) = setup_orchestrator();
        orchestrator.queue().add(test_song("song-0"), None);
        orchestrator.queue().add(test_song("song-1"), None);
        orchestrator.queue().add(test_song("song-2"), None);

        let (event_tx, _event_rx) = crossbeam::channel::unbounded();
        let response = handle_play(&orchestrator, &event_tx);

        assert!(matches!(response, ServerResponse::Ok));
        assert_eq!(orchestrator.queue().current_index(), Some(0));
        assert_eq!(orchestrator.playback().get_playlist_count().unwrap(), 1);
        assert_eq!(
            recording.prepared.lock().clone(),
            vec![("song-0".to_string(), PreloadTier::Immediate)]
        );
        assert!(recording.prefetched.lock().is_empty());
        assert_eq!(recording.activated_windows.lock().clone(), vec![vec!["song-0".to_string()]]);
    }
}
