//! Event handlers for `PlayQueue` events (Layer 2 Bridge).
//!
//! This handler bridges `PlayQueue` state changes to MPV playlist operations.
//! Key responsibility: Keep MPV's playlist synchronized with queue order
//! changes.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::runtime::{Builder, Handle};

use crate::{
    backends::youtube::{
        audio::MpvInput,
        media::{MediaPreparer, PreloadTier},
        services::{PlaybackService, QueueService},
    },
    shared::play_queue::{PlayQueue, QueueEvent, QueueId, RepeatMode},
};

const PREFETCH_WINDOW_SIZE: usize = 3;

pub struct QueueEventHandler {
    playback: Arc<PlaybackService>,
    queue: Arc<QueueService>,
    play_queue: Arc<Mutex<PlayQueue>>,
    media_preparer: Option<Arc<dyn MediaPreparer>>,
}

impl QueueEventHandler {
    #[must_use]
    pub fn new(
        playback: Arc<PlaybackService>,
        queue: Arc<QueueService>,
        play_queue: Arc<Mutex<PlayQueue>>,
    ) -> Self {
        Self { playback, queue, play_queue, media_preparer: None }
    }

    pub fn with_media_preparer(mut self, preparer: Arc<dyn MediaPreparer>) -> Self {
        self.media_preparer = Some(preparer);
        self
    }

    pub fn handle(&mut self, event: QueueEvent) {
        match event {
            QueueEvent::ItemsAdded { ids } => self.handle_items_added(&ids),
            QueueEvent::ItemsRemoved { ids } => self.handle_items_removed(&ids),
            QueueEvent::OrderChanged { play_order, current_id } => {
                self.handle_order_changed(&play_order, current_id);
            }
            QueueEvent::CurrentChanged { from, to } => self.handle_current_changed(from, to),
            QueueEvent::ModesChanged { shuffle, repeat } => {
                self.handle_modes_changed(shuffle, repeat);
            }
            QueueEvent::Cleared => self.handle_cleared(),
            QueueEvent::Stopped => self.handle_stopped(),
        }
    }

    fn handle_items_added(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsAdded: {ids:?}");

        let play_queue = self.play_queue.lock();
        let video_ids: Vec<String> = ids
            .iter()
            .filter_map(|&id| play_queue.get_song(id).map(|s| s.uri.clone()))
            .filter(|uri| !uri.is_empty())
            .collect();
        drop(play_queue);

        if video_ids.is_empty() {
            return;
        }

        if let Some(ref preparer) = self.media_preparer {
            for uri in &video_ids {
                let Some(track_id) = extract_video_id(uri) else {
                    continue;
                };
                preparer.prefetch(&track_id, PreloadTier::Background);
            }
        } else {
            log::warn!("QueueEventHandler missing media preparer; skipping background prefetch");
        }
    }

    fn handle_items_removed(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsRemoved: {ids:?}");
    }

    fn handle_order_changed(&mut self, play_order: &[QueueId], current_id: Option<QueueId>) {
        log::debug!("QueueEvent::OrderChanged: {} items, current={current_id:?}", play_order.len());

        let mpv_playlist_len = match self.playback.get_playlist_count() {
            Ok(len) => len,
            Err(e) => {
                log::warn!("Failed to get MPV playlist count: {e}");
                return;
            }
        };

        for i in (1..mpv_playlist_len).rev() {
            if let Err(e) = self.playback.playlist_remove(i) {
                log::warn!("Failed to remove MPV playlist item {i}: {e}");
            }
        }

        let current_pos = current_id.and_then(|id| play_order.iter().position(|&x| x == id));

        if let Some(current_pos) = current_pos {
            let window_end = std::cmp::min(current_pos + PREFETCH_WINDOW_SIZE, play_order.len());

            for (offset, &id) in play_order[current_pos + 1..window_end].iter().enumerate() {
                let tier = if offset == 0 { PreloadTier::Gapless } else { PreloadTier::Eager };
                if let Some(input) = self.resolve_playback_input(id, tier) {
                    if let Err(e) = self.playback.playlist_append_input(&input) {
                        log::warn!("Failed to append track {id} to MPV playlist: {e}");
                    } else {
                        log::debug!("Appended track {id} to MPV playlist");
                    }
                }
            }

            self.update_prefetch_window(play_order, current_id);
        } else if !play_order.is_empty() {
            let window_end = std::cmp::min(PREFETCH_WINDOW_SIZE, play_order.len());
            for (offset, &id) in play_order[..window_end].iter().enumerate() {
                let tier = match offset {
                    0 => PreloadTier::Immediate,
                    1 => PreloadTier::Gapless,
                    _ => PreloadTier::Eager,
                };
                if let Some(input) = self.resolve_playback_input(id, tier) {
                    if let Err(e) = self.playback.playlist_append_input(&input) {
                        log::warn!("Failed to append track {id} to MPV playlist: {e}");
                    }
                }
            }
        }
    }

    fn resolve_playback_input(&self, id: QueueId, tier: PreloadTier) -> Option<MpvInput> {
        let play_queue = self.play_queue.lock();
        let song = play_queue.get_song(id)?;
        let video_id = song.uri.clone();
        if video_id.is_empty() {
            log::warn!("Song {id} has empty video_id");
            return None;
        }
        drop(play_queue);

        let Some(preparer) = self.media_preparer.as_ref() else {
            log::warn!("QueueEventHandler missing media preparer; cannot resolve media for {video_id}");
            return None;
        };

        prepare_media_blocking(preparer, &video_id, tier)
            .and_then(|prepared| self.playback.build_runtime_input(&video_id, &prepared))
            .map_err(|e| {
                log::warn!("Failed to prepare playback input for {video_id}: {e}");
                e
            })
            .ok()
    }

    fn update_prefetch_window(&self, play_order: &[QueueId], current_id: Option<QueueId>) {
        let Some(current_pos) = current_id.and_then(|id| play_order.iter().position(|&x| x == id))
        else {
            return;
        };

        let window_end = std::cmp::min(current_pos + PREFETCH_WINDOW_SIZE, play_order.len());
        let window = &play_order[current_pos..window_end];
        log::debug!("New prefetch window: {:?}", window);

        let play_queue = self.play_queue.lock();
        let video_ids: Vec<String> = window
            .iter()
            .filter_map(|&id| play_queue.get_song(id).map(|s| s.uri.clone()))
            .filter(|uri| !uri.is_empty())
            .collect();
        drop(play_queue);

        if video_ids.is_empty() {
            return;
        }

        if let Some(ref preparer) = self.media_preparer {
            preparer.activate_playback_window(&video_ids);

            for (index, uri) in video_ids.iter().enumerate() {
                let Some(track_id) = extract_video_id(uri) else {
                    continue;
                };

                let tier = match index {
                    0 => PreloadTier::Immediate,
                    1 => PreloadTier::Gapless,
                    _ => PreloadTier::Eager,
                };

                preparer.prefetch(&track_id, tier);
            }
        } else {
            log::warn!("QueueEventHandler missing media preparer; skipping prefetch window update");
        }
    }

    fn handle_current_changed(&mut self, from: Option<QueueId>, to: Option<QueueId>) {
        log::debug!("QueueEvent::CurrentChanged: {from:?} -> {to:?}");
    }

    fn handle_modes_changed(&mut self, shuffle: bool, repeat: RepeatMode) {
        log::debug!("QueueEvent::ModesChanged: shuffle={shuffle}, repeat={repeat:?}");
    }

    fn handle_cleared(&mut self) {
        log::debug!("QueueEvent::Cleared");
        if let Some(ref preparer) = self.media_preparer {
            preparer.activate_playback_window(&[]);
        }
    }

    fn handle_stopped(&mut self) {
        log::debug!("QueueEvent::Stopped");
        if let Some(ref preparer) = self.media_preparer {
            preparer.activate_playback_window(&[]);
        }
    }
}

fn extract_video_id(uri: &str) -> Option<String> {
    if let Some(id) = uri.strip_prefix("youtube://") {
        return Some(id.to_string());
    }

    if !uri.is_empty() && !uri.contains("://") {
        return Some(uri.to_string());
    }

    None
}

fn prepare_media_blocking(
    media_preparer: &Arc<dyn MediaPreparer>,
    track_id: &str,
    tier: PreloadTier,
) -> anyhow::Result<crate::backends::youtube::media::PreparedMedia> {
    if let Ok(handle) = Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(media_preparer.prepare(track_id, tier)))
    } else {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(media_preparer.prepare(track_id, tier))
    }
}
