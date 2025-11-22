pub mod backend;
pub mod client;
pub mod mpd_backend;
pub mod mpv_backend;
pub mod mpv_ipc;

// Re-export for convenience
pub use client::Client;
