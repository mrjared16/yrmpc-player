
use anyhow::Result;

use crate::{
    domain::{Song, Status, QueuePosition},
    mpd::{SingleOrRange, commands::*, mpd_client::Filter, version::Version},
};

/// Trait for music player backends (MPD, MPV, etc.)
///
/// This trait abstracts different music player backends to allow rmpc
/// to work with multiple players (MPD, MPV, etc.)
pub trait MusicBackend: Send + Sync {
    // ===== Playback Control =====

    fn play(&mut self) -> Result<()>;
    fn pause(&mut self, state: bool) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<()>;
    fn previous(&mut self) -> Result<()>;
    fn seek_current(&mut self, position: SeekPosition) -> Result<()>;

    // ===== Status Queries =====

    fn get_status(&mut self) -> Result<Status>;
    fn playlist_info(&mut self) -> Result<Vec<Song>>;
    fn current_song(&mut self) -> Result<Option<Song>>;

    // ===== Queue Management =====

    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()>;
    fn delete_id(&mut self, id: u32) -> Result<()>;
    fn clear(&mut self) -> Result<()>;
    fn move_id(&mut self, from: u32, to: u32) -> Result<()>;
    fn play_id(&mut self, id: u32) -> Result<()>;

    // ===== Volume Control =====

    fn volume(&mut self) -> Result<u8>;
    fn set_volume(&mut self, volume: ValueChange) -> Result<()>;

    // ===== Playback Options =====

    fn repeat(&mut self, repeat: bool) -> Result<()>;
    fn random(&mut self, random: bool) -> Result<()>;
    fn single(&mut self, single: OnOffOneshot) -> Result<()>;
    fn consume(&mut self, consume: OnOffOneshot) -> Result<()>;
    fn crossfade(&mut self, seconds: u32) -> Result<()>;
    fn shuffle(&mut self, range: Option<SingleOrRange>) -> Result<()>;

    // ===== Library Browsing =====

    fn get_search_suggestions(&mut self, query: String) -> Result<Vec<String>>;
    
    /// Get library data for a specific category (playlists, albums, artists, songs)
    /// Returns Vec<LsInfoEntry> containing directories or songs depending on category
    fn get_library(&mut self, category: super::LibraryCategory) -> Result<Vec<LsInfoEntry>>;

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>>;
    fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>>;
    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>>;
    fn find(&mut self, filter: &[Filter], window: Option<(u32, u32)>) -> Result<Vec<Song>>;
    fn list_tag(&mut self, tag: Tag, filter: Option<&[Filter]>) -> Result<Vec<String>>;
    fn count(&mut self, filter: &[Filter]) -> Result<(usize, std::time::Duration)>;

    // ===== Playlist Management =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>>;
    fn playlist_info_name(&mut self, name: &str) -> Result<Vec<Song>>;
    fn load_playlist(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()>;
    fn save_queue_as_playlist(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()>;
    fn delete_playlist(&mut self, name: &str) -> Result<()>;
    fn rename_playlist(&mut self, old_name: &str, new_name: &str) -> Result<()>;
    fn add_to_playlist(&mut self, playlist: &str, uri: &str) -> Result<()>;
    fn delete_from_playlist(&mut self, playlist: &str, position: u32) -> Result<()>;
    fn move_in_playlist(&mut self, playlist: &str, from: SingleOrRange, to: u32) -> Result<()>;

    // ===== Sticker Support =====

    fn list_stickers(&mut self, uri: &str) -> Result<std::collections::HashMap<String, String>>;
    fn set_sticker(&mut self, uri: &str, key: &str, value: &str) -> Result<()>;
    fn delete_sticker(&mut self, uri: &str, key: &str) -> Result<()>;

    // ===== Database Management =====

    fn update(&mut self, path: Option<&str>) -> Result<u32>;
    fn rescan(&mut self, path: Option<&str>) -> Result<u32>;

    // ===== System Info =====

    fn version(&self) -> Version;
    fn outputs(&mut self) -> Result<Vec<Output>>;
    fn decoders(&mut self) -> Result<Vec<Decoder>>;
    fn partitions(&mut self) -> Result<Vec<String>>;

    // ===== Backend Identification =====

    /// Returns the name of this backend (e.g., "MPD", "MPV")
    fn backend_name(&self) -> &'static str;

    /// Returns whether this backend supports a specific command
    fn supports_command(&self, _command: &str) -> bool {
        // Default: assume all commands are supported
        // Backends can override this
        true
    }
}
