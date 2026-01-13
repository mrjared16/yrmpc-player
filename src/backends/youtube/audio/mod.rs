//! Audio streaming module for YouTube backend.
//!
//! Provides pluggable audio source architecture with prefix caching.
//! See ADR-001 for architecture decisions.
//!
//! ## Architecture
//!
//! ```text
//! PlaybackService → MpvAudioSource → AudioCache → MPV
//!                        ↓
//!                  ConcatSource (default)
//!                        ↓
//!                  concat URL
//! ```
//!
//! ## Components
//!
//! - [`MpvAudioSource`] - Trait for pluggable audio sources
//! - [`ConcatSource`] - Default implementation using ffmpeg concat+subfile
//! - [`AudioCache`] - Prefix file management with LRU eviction

pub mod cache;
pub mod mpv_source;
pub mod range_set;
pub mod sources;

pub use cache::{AudioCache, CacheConfig};
pub use mpv_source::{MpvAudioSource, MpvInput};
pub use range_set::RangeSet;

#[cfg(test)]
mod tests;
