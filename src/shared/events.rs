use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use crossterm::event::KeyEvent;
use serde::{Deserialize, Serialize};

use super::{
    ipc::ipc_stream::IpcStream,
    lrc::{LrcIndex, LrcIndexEntry},
    mouse_event::MouseEvent,
    ytdlp::{SearchItem, YtDlpHostKind},
};
// Re-export ClientRequest from backends::messaging for internal use
pub(crate) use crate::backends::messaging::ClientRequest;
use crate::{
    backends::messaging::{PlayerCommand, Query, QueryResult, QuerySync},
    config::{
        Config,
        Size,
        cli::{Command, RemoteCommandQuery},
        tabs::PaneType,
        theme::UiConfig,
    },
    mpd::{QueuePosition, commands::IdleEvent},
    ui::UiAppEvent,
};

#[derive(Debug)]
#[allow(unused)]
pub(crate) enum WorkRequest {
    IndexLyrics {
        lyrics_dir: String,
    },
    IndexSingleLrc {
        /// Absolute path to the lrc file
        path: PathBuf,
    },
    SearchYt {
        query: String,
        kind: YtDlpHostKind,
        limit: usize,
        interactive: bool,
        position: Option<QueuePosition>,
    },
    Command(Command), // cli::Command for remote commands
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // the instances are short lived events, its fine.
pub(crate) enum WorkDone {
    LyricsIndexed { index: LrcIndex },
    SingleLrcIndexed { lrc_entry: Option<LrcIndexEntry> },
    QueryFinished { id: &'static str, target: Option<PaneType>, data: QueryResult },
    SearchYtResults { items: Vec<SearchItem>, position: Option<QueuePosition> },
    None,
}

// The instances are short lived events, boxing would most likely only hurt
// here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum AppEvent {
    UserKeyInput(KeyEvent),
    UserMouseInput(MouseEvent),
    Status(String, Level, Duration),
    InfoModal {
        message: Vec<String>,
        title: Option<String>,
        size: Option<Size>,
        replacement_id: Option<String>,
    },
    Log(Vec<u8>),
    IdleEvent(IdleEvent),
    RequestRender,
    Resized {
        columns: u16,
        rows: u16,
    },
    ResizedDebounced {
        columns: u16,
        rows: u16,
    },
    WorkDone(Result<WorkDone>),
    UiEvent(UiAppEvent),
    Reconnected,
    LostConnection,
    TmuxHook {
        hook: String,
    },
    ConfigChanged {
        config: Box<Config>,
        keep_old_theme: bool,
    },
    ThemeChanged {
        theme: Box<UiConfig>,
    },
    RemoteSwitchTab {
        tab_name: String,
    },
    IpcQuery {
        stream: IpcStream,
        targets: Vec<RemoteCommandQuery>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, Eq, Hash, PartialEq)]
#[allow(dead_code)]
pub enum Level {
    Trace,
    Debug,
    Warn,
    Error,
    Info,
}
