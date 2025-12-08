pub mod count;
pub mod current_song;
pub mod decoders;
pub mod idle;
pub mod list;
pub mod list_all;
pub mod list_files;
pub mod list_mounts;
pub mod list_playlist;
pub mod list_playlists;
pub mod lsinfo;
pub mod metadata_tag;
pub mod mpd_config;
pub mod outputs;
pub mod playlist_info;
pub mod status;
pub mod stickers;
pub mod update;
pub mod volume;

pub use self::{
    current_song::Song,
    decoders::Decoder,
    idle::IdleEvent,
    list_files::ListFiles,
    list_mounts::Mounts,
    list_playlists::Playlist,
    lsinfo::{LsInfo, LsInfoEntry},
    outputs::Output,
    status::{OnOffOneshot, State, Status},
    update::Update,
    volume::Volume,
};
// Re-export types from parent modules
pub use crate::mpd::{
    mpd_client::{SaveMode, Tag, ValueChange},
    queue_position::QueuePosition,
};

/// Position for seeking in a track
#[derive(Debug, Clone, Copy)]
pub enum SeekPosition {
    /// Absolute position in seconds
    Absolute(f64),
    /// Relative position in seconds (positive or negative)
    Relative(f64),
}
