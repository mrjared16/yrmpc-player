//! Audio source implementations.
//!
//! Provides different strategies for building MPV input URLs.
//!
//! ## Available Sources
//!
//! - `ConcatSource` (DEFAULT): Uses ffmpeg concat+subfile protocol for byte-perfect playback
//! - `ProxySource` (FUTURE): HTTP server for offline mode, metrics, URL refresh

pub mod concat;

pub use concat::ConcatSource;
