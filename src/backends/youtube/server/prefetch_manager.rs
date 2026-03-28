use std::{collections::HashSet, sync::Arc};

use parking_lot::Mutex;

use crate::backends::youtube::{
    media::{MediaPreparer, PreloadTier},
    server::handlers::stable_track_id,
    services::QueueService,
};

const PREFETCH_TRIGGER_THRESHOLD: f64 = 30.0;

pub struct PrefetchManager {
    queue: Arc<QueueService>,
    media_preparer: Arc<dyn MediaPreparer>,
    triggered: Arc<Mutex<HashSet<String>>>,
}

impl PrefetchManager {
    pub fn new(queue: Arc<QueueService>, media_preparer: Arc<dyn MediaPreparer>) -> Self {
        Self { queue, media_preparer, triggered: Arc::new(Mutex::new(HashSet::new())) }
    }

    pub fn handle_time_remaining(&self, time_remaining_secs: f64) {
        if time_remaining_secs > PREFETCH_TRIGGER_THRESHOLD || time_remaining_secs <= 0.0 {
            return;
        }

        let current_idx = match self.queue.current_index() {
            Some(idx) => idx,
            None => return,
        };

        let current_song = match self.queue.get_by_index(current_idx) {
            Ok(song) => song,
            Err(_) => return,
        };

        let video_id = stable_track_id(&current_song.uri);
        if video_id.is_empty() {
            return;
        }

        {
            let mut triggered = self.triggered.lock();
            if triggered.contains(&video_id) {
                return;
            }
            triggered.insert(video_id.clone());
        }

        let next_idx = match self.queue.next_index() {
            Some(idx) => idx,
            None => return,
        };

        let next_song = match self.queue.get_by_index(next_idx) {
            Ok(song) => song,
            Err(e) => {
                log::warn!("Failed to get next track for prefetch: {}", e);
                return;
            }
        };

        let next_track_id = stable_track_id(&next_song.uri);
        if next_track_id.is_empty() {
            return;
        }

        log::info!(
            "T-30s prefetch trigger: {}s remaining, prefetching next track: {}",
            time_remaining_secs,
            next_track_id
        );

        self.media_preparer.prefetch(&next_track_id, PreloadTier::Gapless);
    }

    pub fn prefetch_upcoming(&self) {
        if let Some(current) = self.queue.current_index() {
            let window = self.queue.build_prefetch_window(current, 6);
            let track_ids: Vec<String> = window
                .iter()
                .skip(1)
                .filter_map(|&idx| self.queue.get_by_index(idx).ok())
                .map(|song| stable_track_id(&song.uri))
                .filter(|track_id| !track_id.is_empty())
                .collect();

            for track_id in track_ids {
                self.media_preparer.prefetch(&track_id, PreloadTier::Background);
            }
        }
    }

    pub fn clear_triggered(&self) {
        let mut triggered = self.triggered.lock();

        let current_idx = match self.queue.current_index() {
            Some(idx) => idx,
            None => {
                triggered.clear();
                return;
            }
        };

        self.queue.ensure_shuffle_order(current_idx);
        let window = self.queue.compute_prefetch_window(current_idx, 4);
        let keep_ids: Vec<String> = window
            .iter()
            .filter_map(|&idx| self.queue.get_by_index(idx).ok())
            .map(|song| stable_track_id(&song.uri))
            .filter(|track_id| !track_id.is_empty())
            .collect();

        triggered.retain(|id| keep_ids.contains(id));
    }

    #[cfg(test)]
    pub(crate) fn mark_triggered_for_test(&self, track_id: String) {
        self.triggered.lock().insert(track_id);
    }

    #[cfg(test)]
    pub(crate) fn is_triggered_for_test(&self, track_id: &str) -> bool {
        self.triggered.lock().contains(track_id)
    }
}
