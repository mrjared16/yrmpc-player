//! Command handlers for YouTube server.
//!
//! Each handler module contains thin wrappers that:
//! 1. Validate input
//! 2. Call orchestrator or services
//! 3. Return ServerResponse
//!
//! Handlers are organized by domain:
//! - `playback`: Play, Pause, Stop, Seek, Next, Previous
//! - `queue`: Add, Delete, Clear, Move
//! - `status`: GetStatus, GetCurrentSong, GetPlaylist
//! - `search`: Search, Browse, GetSearchSuggestions
//! - `options`: SetRepeat, SetShuffle, Volume operations

pub mod playback;
pub mod queue;
pub mod status;
pub mod search;
pub mod options;

pub use playback::*;
pub use queue::*;
pub use status::*;
pub use search::*;
pub use options::*;
