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
    /// Replace current context and start playing.
    /// Priority: tracks[offset]=Immediate, tracks[offset+1]=Gapless,
    /// rest=Background
    Context { tracks: Vec<Song>, offset: usize, shuffle: bool, source: Option<ContextSource> },

    /// Insert tracks to play after current song ends.
    /// Priority: tracks[0]=Gapless, rest=Eager
    Next { tracks: Vec<Song> },

    /// Append tracks to end of queue.
    /// Priority: all=Background
    Append { tracks: Vec<Song> },

    /// Start infinite radio from seed track.
    /// Priority: seed=Immediate, lazy-fetched tracks=Background
    Radio { seed: Song, mix_type: MixType },
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
/// - **Context**: tracks[offset]=Immediate, tracks[offset+1]=Gapless,
///   rest=Background
/// - **Next**: tracks[0]=Gapless, rest=Eager
/// - **Append**: all=Background
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
/// let intent = PlayIntent::Context {
///     tracks: songs.clone(),
///     offset: 0,
///     shuffle: false,
///     source: None,
/// };
///
/// let priorities = derive_priorities(&intent);
/// assert_eq!(priorities[0].1, PreloadTier::Immediate); // First track
/// assert_eq!(priorities[1].1, PreloadTier::Gapless);   // Second track
/// assert_eq!(priorities[2].1, PreloadTier::Background); // Rest
/// ```
pub fn derive_priorities(intent: &PlayIntent) -> Vec<(Song, PreloadTier)> {
    match intent {
        PlayIntent::Context { tracks, offset, shuffle: _, source: _ } => tracks
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let tier = match i.cmp(offset) {
                    std::cmp::Ordering::Equal => PreloadTier::Immediate,
                    std::cmp::Ordering::Greater if i == offset + 1 => PreloadTier::Gapless,
                    _ => PreloadTier::Background,
                };
                (song.clone(), tier)
            })
            .collect(),

        PlayIntent::Next { tracks } => tracks
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let tier = if i == 0 { PreloadTier::Gapless } else { PreloadTier::Eager };
                (song.clone(), tier)
            })
            .collect(),

        PlayIntent::Append { tracks } => {
            tracks.iter().map(|song| (song.clone(), PreloadTier::Background)).collect()
        }

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
        let intent =
            PlayIntent::Context { tracks: songs.clone(), offset: 0, shuffle: false, source: None };

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
        let intent =
            PlayIntent::Context { tracks: songs.clone(), offset: 2, shuffle: false, source: None };

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
        let intent = PlayIntent::Next { tracks: songs.clone() };

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 3);
        assert_eq!(priorities[0].1, PreloadTier::Gapless);
        assert_eq!(priorities[1].1, PreloadTier::Eager);
        assert_eq!(priorities[2].1, PreloadTier::Eager);
    }

    #[test]
    fn test_derive_priorities_append() {
        let songs = vec![test_song("s1"), test_song("s2")];
        let intent = PlayIntent::Append { tracks: songs.clone() };

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
        let intent =
            PlayIntent::Context { tracks: vec![], offset: 0, shuffle: false, source: None };

        let priorities = derive_priorities(&intent);
        assert_eq!(priorities.len(), 0);
    }

    #[test]
    fn test_derive_priorities_single_track_context() {
        let songs = vec![test_song("s1")];
        let intent =
            PlayIntent::Context { tracks: songs.clone(), offset: 0, shuffle: false, source: None };

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
