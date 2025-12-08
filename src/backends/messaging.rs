//! Backend messaging types for queries and commands.
//!
//! This module defines the protocol for communicating between the UI thread
//! and the backend thread. It includes:
//! - `Query`: Async requests that return data
//! - `QuerySync`: Synchronous requests with blocking response
//! - `PlayerCommand`: Fire-and-forget commands
//! - `QueryResult`: Enum of all possible query responses
//! - `ClientRequest`: Unified request type for the backend channel

use std::{any::Any, collections::HashMap, sync::Arc};

use anyhow::Result;
use bon::Builder;
use crossbeam::channel::Sender;
use ratatui::{style::Style, widgets::ListItem};

use crate::{
    config::tabs::PaneType,
    domain::{Song, Status},
    mpd::commands::{Decoder, IdleEvent, Volume},
    shared::macros::try_skip,
    ui::{dir_or_song::DirOrSong, dirstack::Path},
};

// Forward import for BackendDispatcher
use super::BackendDispatcher;

// Re-export from interaction module
pub use super::interaction::PartitionedOutput;

pub const EXTERNAL_COMMAND: &str = "external_command";
pub const GLOBAL_STATUS_UPDATE: &str = "global_status_update";
pub const GLOBAL_VOLUME_UPDATE: &str = "global_volume_update";
pub const GLOBAL_QUEUE_UPDATE: &str = "global_queue_update";
pub const GLOBAL_STICKERS_UPDATE: &str = "global_stickers_update";

/// A query to be executed by the backend thread
#[derive(derive_more::Debug, Builder)]
pub struct Query {
    pub id: &'static str,
    pub replace_id: Option<&'static str>,
    pub target: Option<PaneType>,
    #[debug(skip)]
    pub callback: Box<dyn FnOnce(&mut BackendDispatcher<'_>) -> Result<QueryResult> + Send>,
}

/// A synchronous query that blocks until the result is available
#[derive(derive_more::Debug, Builder)]
pub struct QuerySync {
    #[debug(skip)]
    pub callback: Box<dyn FnOnce(&mut BackendDispatcher<'_>) -> Result<QueryResult> + Send>,
    pub tx: Sender<QueryResult>,
}

/// A command to be executed by the backend (fire-and-forget)
#[derive(derive_more::Debug)]
pub struct PlayerCommand {
    #[debug(skip)]
    pub callback: Box<dyn FnOnce(&mut BackendDispatcher<'_>) -> Result<()> + Send>,
}

impl Query {
    pub(crate) fn should_be_skipped(&self, other: &Self) -> bool {
        let Some(self_replace_id) = self.replace_id else {
            return false;
        };
        let Some(other_replace_id) = other.replace_id else {
            return false;
        };

        return self.id == other.id
            && self_replace_id == other_replace_id
            && self.target == other.target;
    }
}

/// A group of preview items with optional header
#[derive(Debug, Clone, Default)]
pub struct PreviewGroup {
    pub name: Option<&'static str>,
    pub items: Vec<ListItem<'static>>,
    pub header_style: Option<Style>,
}

impl PreviewGroup {
    pub fn new(name: Option<&'static str>, header_style: Option<Style>) -> Self {
        Self { name, items: Vec::new(), header_style }
    }

    pub fn push(&mut self, item: ListItem<'static>) {
        self.items.push(item);
    }
}

/// Result types for backend queries
#[derive(Debug)]
#[allow(unused, clippy::large_enum_variant)]
pub enum QueryResult {
    SongsList { data: Vec<Song>, path: Option<Path> },
    LsInfo { data: Vec<String>, path: Option<Path> },
    DirOrSong { data: Vec<DirOrSong>, path: Option<Path> },
    SearchSuggestions(Vec<String>),
    SearchResult { data: Vec<Song> },
    AddToPlaylist { playlists: Vec<String>, song_file: String },
    AddToPlaylistMultiple { playlists: Vec<String>, song_files: Vec<String> },
    AlbumArt(Option<Vec<u8>>),
    Status { data: Status, source_event: Option<IdleEvent> },
    Queue(Option<Vec<Song>>),
    Volume(Volume),
    Outputs(Vec<PartitionedOutput>),
    Decoders(Vec<Decoder>),
    ExternalCommand(Arc<Vec<String>>, Vec<Song>),
    SongStickers(HashMap<String, HashMap<String, String>>),
    PlaylistDetail(crate::backends::youtube::PlaylistDetails),
    AlbumDetail(crate::backends::youtube::AlbumDetails),
    ArtistDetail(crate::backends::youtube::ArtistDetails),
    Any(Box<dyn Any + Send + Sync>),
}

/// Unified request type for the backend channel
#[derive(Debug)]
#[allow(unused)]
pub enum ClientRequest {
    Query(Query),
    QuerySync(QuerySync),
    Command(PlayerCommand),
}

/// Event types used by the application
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Copy, Eq, Hash, PartialEq)]
#[allow(dead_code)]
pub enum Level {
    Trace,
    Debug,
    Warn,
    Error,
    Info,
}

// Scheduled function for status updates
use crate::shared::events::AppEvent;

/// Scheduled function to send periodic status updates
#[allow(clippy::unnecessary_wraps)]
pub fn run_status_update((_, client_tx): &(Sender<AppEvent>, Sender<ClientRequest>)) -> Result<()> {
    try_skip!(
        client_tx.send(ClientRequest::Query(Query {
            id: GLOBAL_STATUS_UPDATE,
            target: None,
            replace_id: Some("status"),
            callback: Box::new(move |client| Ok(QueryResult::Status {
                data: client.get_status()?,
                source_event: None
            })),
        })),
        "Failed to send status update query"
    );
    Ok(())
}
