pub mod backend;
pub mod client;
pub mod mpd_backend;
pub mod mpv_backend;
pub mod mpv_ipc;

// Re-export for convenience
pub use backend::MusicBackend;
pub use client::Client;
pub use mpd_backend::MpdBackend;
pub use mpv_backend::MpvBackend;
