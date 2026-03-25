use crate::{
    backends::youtube::{server::handlers::stable_track_id, services::QueueService},
    shared::play_queue::{PlayQueue, QueueId},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPlaybackHorizon {
    track_ids: Vec<String>,
}

impl ResolvedPlaybackHorizon {
    #[must_use]
    pub fn new(track_ids: Vec<String>) -> Self {
        Self { track_ids }
    }

    #[must_use]
    pub fn from_queue_service(queue: &QueueService, start_idx: usize) -> Self {
        Self::new(
            queue
                .compute_prefetch_window(start_idx, queue.len())
                .into_iter()
                .filter_map(|idx| queue.get_by_index(idx).ok())
                .map(|song| stable_track_id(&song.uri))
                .filter(|track_id| !track_id.is_empty())
                .collect(),
        )
    }

    #[must_use]
    pub fn from_play_queue(
        play_queue: &PlayQueue,
        play_order: &[QueueId],
        current_id: Option<QueueId>,
    ) -> Self {
        let current_track = current_id
            .and_then(|id| play_queue.get_song(id).map(|song| stable_track_id(&song.uri)))
            .filter(|track_id| !track_id.is_empty());

        Self::from_play_queue_track_id(play_queue, play_order, current_track.as_deref())
    }

    #[must_use]
    pub fn from_play_queue_track_id(
        play_queue: &PlayQueue,
        play_order: &[QueueId],
        current_track: Option<&str>,
    ) -> Self {
        if play_order.is_empty() {
            return Self::default();
        }

        let track_ids: Vec<String> = play_order
            .iter()
            .filter_map(|&id| play_queue.get_song(id).map(|song| stable_track_id(&song.uri)))
            .filter(|track_id| !track_id.is_empty())
            .collect();

        if track_ids.is_empty() {
            return Self::default();
        }

        let start_pos = current_track
            .and_then(|track_id| track_ids.iter().position(|candidate| candidate == track_id))
            .unwrap_or(0);

        Self::new(
            track_ids[start_pos..]
                .iter()
                .chain(track_ids[..start_pos].iter())
                .cloned()
                .collect(),
        )
    }

    #[must_use]
    pub fn track_ids(&self) -> &[String] {
        &self.track_ids
    }

    #[must_use]
    pub fn next_tracks_after(&self, current_track: Option<&str>, count: usize) -> Vec<String> {
        match current_track {
            Some(current_track) => {
                let Some(start_index) = self
                    .track_ids
                    .iter()
                    .position(|track_id| track_id == current_track)
                    .map(|index| index.saturating_add(1))
                else {
                    return Vec::new();
                };

                self.track_ids
                    .iter()
                    .skip(start_index)
                    .filter(|track_id| track_id.as_str() != current_track)
                    .take(count)
                    .cloned()
                    .collect()
            }
            None => self.track_ids.iter().take(count).cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        backends::youtube::services::QueueService,
        shared::play_queue::{PlayQueue, QueueCommand},
    };
    use crate::domain::Song;

    use super::ResolvedPlaybackHorizon;

    fn test_song(uri: &str) -> Song {
        Song { uri: uri.to_string(), ..Song::default() }
    }

    #[test]
    fn next_tracks_after_excludes_current_track() {
        let horizon = ResolvedPlaybackHorizon::new(vec![
            "current".into(),
            "next-1".into(),
            "next-2".into(),
            "next-3".into(),
        ]);

        assert_eq!(
            horizon.next_tracks_after(Some("current"), 3),
            vec!["next-1", "next-2", "next-3"]
        );
    }

    #[test]
    fn next_tracks_after_returns_empty_when_current_track_is_missing() {
        let horizon = ResolvedPlaybackHorizon::new(vec!["current".into(), "next-1".into()]);

        assert!(horizon.next_tracks_after(Some("missing"), 3).is_empty());
    }

    #[test]
    fn from_queue_service_uses_queue_prefetch_order() {
        let queue = QueueService::new();
        queue.add(test_song("youtube://a"), None);
        queue.add(test_song("youtube://b"), None);
        queue.add(test_song("youtube://c"), None);

        let horizon = ResolvedPlaybackHorizon::from_queue_service(&queue, 1);
        assert_eq!(horizon.track_ids(), ["b", "c"]);
    }

    #[test]
    fn from_play_queue_rotates_from_current_id() {
        let mut play_queue = PlayQueue::new();
        let a = match play_queue.apply(QueueCommand::Add { song: test_song("youtube://a") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        let b = match play_queue.apply(QueueCommand::Add { song: test_song("youtube://b") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        let c = match play_queue.apply(QueueCommand::Add { song: test_song("youtube://c") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { ref ids } => ids[0],
            _ => unreachable!(),
        };
        let play_order = play_queue.get_play_order().to_vec();
        play_queue.apply(QueueCommand::Play { id: b });

        let horizon = ResolvedPlaybackHorizon::from_play_queue(&play_queue, &play_order, Some(b));
        assert_eq!(horizon.track_ids(), ["b", "c", "a"]);
        assert_ne!(a, c);
    }

    #[test]
    fn from_play_queue_track_id_rotates_from_current_track() {
        let mut play_queue = PlayQueue::new();
        match play_queue.apply(QueueCommand::Add { song: test_song("youtube://a") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { .. } => {}
            _ => unreachable!(),
        }
        match play_queue.apply(QueueCommand::Add { song: test_song("youtube://b") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { .. } => {}
            _ => unreachable!(),
        }
        match play_queue.apply(QueueCommand::Add { song: test_song("youtube://c") })[0] {
            crate::shared::play_queue::QueueEvent::ItemsAdded { .. } => {}
            _ => unreachable!(),
        }

        let play_order = play_queue.get_play_order().to_vec();
        let horizon = ResolvedPlaybackHorizon::from_play_queue_track_id(
            &play_queue,
            &play_order,
            Some("b"),
        );

        assert_eq!(horizon.track_ids(), ["b", "c", "a"]);
    }
}
