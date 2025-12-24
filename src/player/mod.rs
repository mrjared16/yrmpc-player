//! Player module - DEPRECATED, use `crate::backends` instead.
//!
//! This module exists only for backward compatibility.
//! All implementations are now in `crate::backends`.

// Re-export everything from backends for backward compatibility
pub use crate::backends::{
    BackendDispatcher,
    LibraryCategory,
    MpdBackend,
    MpvIpc, MpvEvent,  // MPV is now internal to YouTube backend
};
