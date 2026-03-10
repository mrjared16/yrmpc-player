//! Audio streaming module for YouTube backend.
//!
//! Provides pluggable audio source architecture with prefix caching.
//! See ADR-001 for architecture decisions.

pub mod cache;
pub mod mpv_source;
pub mod planner;
pub mod sources;

pub use cache::{AudioCache, CacheConfig};
pub use mpv_source::{MpvAudioSource, MpvInput};
pub use planner::{
    AudioSourcePlan,
    AudioSourcePlanner,
    AudioTransportTarget,
    PrefetchPolicy,
    PrepareAction,
};
pub use sources::{FfmpegConcatSource, PassthroughSource};

#[cfg(test)]
mod tests;
