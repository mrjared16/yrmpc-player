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

pub mod options;
pub mod play_intent;
pub mod playback;
pub mod queue;
pub mod queue_events;
pub mod search;
pub mod status;

pub use options::*;
pub use play_intent::*;
pub use playback::*;
pub use queue::*;
pub use search::*;
pub use status::*;
