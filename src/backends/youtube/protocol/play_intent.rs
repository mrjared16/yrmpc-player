//! PlayIntent types for declarative playback control.
//!
//! This module defines the core types for the PlayIntent architecture,
//! which enables low-latency playback by expressing user intent declaratively.

use serde::{Deserialize, Serialize};

use crate::domain::Song;

/// Request identifier for tracking and cancellation.
pub type RequestId = u64;

/// Declarative user intent for playback operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlayIntent {
    Replace(ReplaceQueue),
    Insert(InsertTracks),

    /// Start infinite radio from seed track.
    /// Priority: seed=Immediate, lazy-fetched tracks=Background
    Radio {
        seed: Song,
        mix_type: MixType,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceQueue {
    pub tracks: Vec<Song>,
    pub playback: ReplacePlayback,
    pub order: QueueOrder,
    pub source: Option<ContextSource>,
    pub target: QueueTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertTracks {
    pub tracks: Vec<Song>,
    pub placement: InsertPlacement,
    pub playback: InsertPlayback,
    pub target: QueueTarget,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ReplacePlayback {
    DoNotStart,
    StartAtIndex(usize),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum QueueOrder {
    Sequential,
    Shuffle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InsertPlacement {
    End,
    AfterCurrent,
    Absolute(usize),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InsertPlayback {
    KeepCurrent,
    StartInserted { index: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueTarget {
    Main,
    Temporary(TemporaryQueueSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryQueueSpec {
    pub on_exhausted: TemporaryQueueExit,
    pub repeat_behavior: TemporaryRepeatBehavior,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TemporaryQueueExit {
    Stop,
    ResumeMainQueue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TemporaryRepeatBehavior {
    FollowPlayerRepeatMode,
    IgnoreRepeatAll,
}

impl PlayIntent {
    pub fn add_last(tracks: Vec<Song>) -> Self {
        Self::Insert(InsertTracks {
            tracks,
            placement: InsertPlacement::End,
            playback: InsertPlayback::KeepCurrent,
            target: QueueTarget::Main,
        })
    }

    pub fn add_next(tracks: Vec<Song>) -> Self {
        Self::Insert(InsertTracks {
            tracks,
            placement: InsertPlacement::AfterCurrent,
            playback: InsertPlayback::KeepCurrent,
            target: QueueTarget::Main,
        })
    }

    pub fn add_at(tracks: Vec<Song>, position: usize) -> Self {
        Self::Insert(InsertTracks {
            tracks,
            placement: InsertPlacement::Absolute(position),
            playback: InsertPlayback::KeepCurrent,
            target: QueueTarget::Main,
        })
    }

    pub fn play_next(tracks: Vec<Song>) -> Self {
        Self::Insert(InsertTracks {
            tracks,
            placement: InsertPlacement::AfterCurrent,
            playback: InsertPlayback::StartInserted { index: 0 },
            target: QueueTarget::Main,
        })
    }

    pub fn play_last(tracks: Vec<Song>) -> Self {
        Self::Insert(InsertTracks {
            tracks,
            placement: InsertPlacement::End,
            playback: InsertPlayback::StartInserted { index: 0 },
            target: QueueTarget::Main,
        })
    }

    pub fn replace_and_play(
        tracks: Vec<Song>,
        start_index: usize,
        shuffle: bool,
        source: Option<ContextSource>,
    ) -> Self {
        Self::Replace(ReplaceQueue {
            tracks,
            playback: ReplacePlayback::StartAtIndex(start_index),
            order: if shuffle { QueueOrder::Shuffle } else { QueueOrder::Sequential },
            source,
            target: QueueTarget::Main,
        })
    }
}

/// Priority tier for preload work.
/// Ordered by urgency: Immediate > Gapless > Eager > Background.
/// Note: Enum variants are in reverse order so derive(Ord) produces correct
/// urgency ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PreloadTier {
    /// Opportunistic. Fill idle bandwidth.
    Background,

    /// Likely needed soon (visible queue, hover preview).
    Eager,

    /// Next track for gapless transition. Wait for prefix.
    Gapless,

    /// User is actively waiting. Never wait for prefix.
    Immediate,
}

/// Optional context for analytics/resume (daemon may ignore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextSource {
    Album { album_id: String },
    Playlist { playlist_id: String },
    Artist { artist_id: String },
    Search { query: String },
    History,
    Queue,
}

/// Radio mix type for auto-extending queue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MixType {
    SongRadio,
    ArtistRadio,
    GenreRadio,
}

/// Errors that can occur during play intent processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlayError {
    EmptyTracks,
    InvalidOffset { offset: usize, len: usize },
    UnsupportedQueueTarget,
    NetworkTimeout,
    RadioSeedInvalid,
}

/// Derive preload priorities from user intent.
///
/// This is a pure function that maps PlayIntent to a list of (Song,
/// PreloadTier) pairs. The daemon uses this to schedule preparation work with
/// appropriate urgency.
///
/// # Priority Rules
///
/// - **Replace + StartAtIndex(i)**: tracks[i]=Immediate, tracks[i+1]=Gapless,
///   rest=Background
/// - **Replace + DoNotStart**: all=Background
/// - **Insert + KeepCurrent**: End=Background, AfterCurrent=Gapless/Eager,
///   Absolute=Eager
/// - **Insert + StartInserted**: inserted=Immediate, next=Gapless, rest=Eager
/// - **Radio**: seed=Immediate
///
/// # Examples
///
/// ```
/// # use rmpc::backends::youtube::protocol::play_intent::{PlayIntent, PreloadTier, derive_priorities};
/// # use rmpc::domain::Song;
/// let songs = vec![
///     Song { uri: "s1".into(), ..Default::default() },
///     Song { uri: "s2".into(), ..Default::default() },
///     Song { uri: "s3".into(), ..Default::default() },
/// ];
///
/// let intent = PlayIntent::replace_and_play(songs.clone(), 0, false, None);
///
/// let priorities = derive_priorities(&intent);
/// assert_eq!(priorities[0].1, PreloadTier::Immediate); // First track
/// assert_eq!(priorities[1].1, PreloadTier::Gapless);   // Second track
/// assert_eq!(priorities[2].1, PreloadTier::Background); // Rest
/// ```
pub fn derive_priorities(intent: &PlayIntent) -> Vec<(Song, PreloadTier)> {
    match intent {
        PlayIntent::Replace(ReplaceQueue { tracks, playback, .. }) => tracks
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let tier = match playback {
                    ReplacePlayback::StartAtIndex(offset) => match i.cmp(offset) {
                        std::cmp::Ordering::Equal => PreloadTier::Immediate,
                        std::cmp::Ordering::Greater if i == offset + 1 => PreloadTier::Gapless,
                        _ => PreloadTier::Background,
                    },
                    ReplacePlayback::DoNotStart => PreloadTier::Background,
                };
                (song.clone(), tier)
            })
            .collect(),

        PlayIntent::Insert(InsertTracks { tracks, placement, playback, .. }) => tracks
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let tier = match playback {
                    InsertPlayback::KeepCurrent => match placement {
                        InsertPlacement::End => PreloadTier::Background,
                        InsertPlacement::AfterCurrent => {
                            if i == 0 {
                                PreloadTier::Gapless
                            } else {
                                PreloadTier::Eager
                            }
                        }
                        InsertPlacement::Absolute(_) => PreloadTier::Eager,
                    },
                    InsertPlayback::StartInserted { index } => match i.cmp(index) {
                        std::cmp::Ordering::Equal => PreloadTier::Immediate,
                        std::cmp::Ordering::Greater if i == index + 1 => PreloadTier::Gapless,
                        _ => PreloadTier::Eager,
                    },
                };
                (song.clone(), tier)
            })
            .collect(),

        PlayIntent::Radio { seed, mix_type: _ } => {
            vec![(seed.clone(), PreloadTier::Immediate)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_song(uri: &str) -> Song {
        Song { uri: uri.to_string(), ..Default::default() }
    }

    #[test]
    fn test_derive_priorities_context_offset_0() {
        let songs = vec![test_song("s1"), test_song("s2"), test_song("s3")];
        let intent = PlayIntent::replace_and_play(songs.clone(), 0, false, None);

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 3);
        assert_eq!(priorities[0].0.uri, "s1");
        assert_eq!(priorities[0].1, PreloadTier::Immediate);
        assert_eq!(priorities[1].0.uri, "s2");
        assert_eq!(priorities[1].1, PreloadTier::Gapless);
        assert_eq!(priorities[2].0.uri, "s3");
        assert_eq!(priorities[2].1, PreloadTier::Background);
    }

    #[test]
    fn test_derive_priorities_context_offset_middle() {
        let songs = vec![test_song("s1"), test_song("s2"), test_song("s3"), test_song("s4")];
        let intent = PlayIntent::replace_and_play(songs.clone(), 2, false, None);

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 4);
        assert_eq!(priorities[0].1, PreloadTier::Background);
        assert_eq!(priorities[1].1, PreloadTier::Background);
        assert_eq!(priorities[2].1, PreloadTier::Immediate);
        assert_eq!(priorities[3].1, PreloadTier::Gapless);
    }

    #[test]
    fn test_derive_priorities_next() {
        let songs = vec![test_song("s1"), test_song("s2"), test_song("s3")];
        let intent = PlayIntent::add_next(songs.clone());

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 3);
        assert_eq!(priorities[0].1, PreloadTier::Gapless);
        assert_eq!(priorities[1].1, PreloadTier::Eager);
        assert_eq!(priorities[2].1, PreloadTier::Eager);
    }

    #[test]
    fn test_derive_priorities_append() {
        let songs = vec![test_song("s1"), test_song("s2")];
        let intent = PlayIntent::add_last(songs.clone());

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 2);
        assert_eq!(priorities[0].1, PreloadTier::Background);
        assert_eq!(priorities[1].1, PreloadTier::Background);
    }

    #[test]
    fn test_derive_priorities_radio() {
        let seed = test_song("seed");
        let intent = PlayIntent::Radio { seed: seed.clone(), mix_type: MixType::SongRadio };

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 1);
        assert_eq!(priorities[0].0.uri, "seed");
        assert_eq!(priorities[0].1, PreloadTier::Immediate);
    }

    #[test]
    fn test_derive_priorities_empty_context() {
        let intent = PlayIntent::Replace(ReplaceQueue {
            tracks: vec![],
            playback: ReplacePlayback::DoNotStart,
            order: QueueOrder::Sequential,
            source: None,
            target: QueueTarget::Main,
        });

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 0);
    }

    #[test]
    fn test_derive_priorities_single_track_context() {
        let songs = vec![test_song("s1")];
        let intent = PlayIntent::replace_and_play(songs.clone(), 0, false, None);

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 1);
        assert_eq!(priorities[0].1, PreloadTier::Immediate);
    }

    #[test]
    fn test_preload_tier_ordering() {
        assert!(PreloadTier::Immediate > PreloadTier::Gapless);
        assert!(PreloadTier::Gapless > PreloadTier::Eager);
        assert!(PreloadTier::Eager > PreloadTier::Background);
    }
}
