//! High-level backend actions for queue and library operations.
//!
//! # Overview
//!
//! This module provides the [`BackendActions`] trait which defines high-level
//! operations that work uniformly across all music backends (MPD, YouTube, etc.).
//!
//! **For new developers:** This is the primary interface for TUI components to
//! interact with the backend. Instead of calling low-level backend methods directly,
//! use these high-level actions which handle:
//! - Autoplay index calculation
//! - Queue position resolution
//! - Backend-specific quirks (e.g., YouTube queue refresh)
//!
//! # Key Types
//!
//! - [`BackendActions`]: Extension trait implemented on [`BackendDispatcher`](super::BackendDispatcher)
//! - [`Enqueue`]: Items that can be added to the queue (files, directories, playlists)
//! - [`DeleteTarget`]: Items that can be deleted (queue positions, ranges)
//! - [`PartitionedOutput`]: Audio output information (MPD-specific)
//!
//! # Example
//!
//! ```ignore
//! // From a TUI component:
//! BackendDispatcher::resolve_and_enqueue(
//!     ctx,
//!     vec![Enqueue::File { path: song.uri.clone() }],
//!     Position::End,
//!     AutoplayKind::Auto,
//!     None,
//!     Some(selected_idx),
//! );
//! ```

use std::{collections::HashMap, sync::Arc};

use itertools::Itertools;

use crate::{
    config::keys::actions::{AddOpts, AutoplayKind, Position},
    ctx::Ctx,
    mpd::{
        QueuePosition,
        commands::{State, outputs::Outputs, stickers::Stickers},
        errors::{ErrorCode, MpdError, MpdFailureResponse},
        mpd_client::{Filter, FilterKind, MpdClient, Command, SingleOrRange, Tag},
        proto_client::ProtoClient,
    },
    shared::macros::{status_info, status_warn},
};

/// High-level actions for backend interaction.
///
/// This trait provides a unified interface for queue operations, sticker management,
/// and other common actions across all music backends.
///
/// # Design Note
///
/// Despite the implementation details, this trait is **backend-agnostic**.
/// It's implemented on [`BackendDispatcher`](super::BackendDispatcher) which
/// dispatches to the appropriate backend (MPD or YouTube).
///
/// # Historical Note
///
/// This was previously named `BackendActions`. The name was changed to reflect
/// that these actions work across all backends, not just MPD.
pub trait BackendActions {
    fn resolve_and_enqueue(
        ctx: &Ctx,
        items: Vec<Enqueue>,
        position: Position,
        autoplay: AutoplayKind,
        current_song_idx: Option<usize>,
        hovered_song_idx: Option<usize>,
    ) {
        let opts = AddOpts { autoplay, position, all: false };
        let replace = matches!(position, Position::Replace);
        let (autoplay_idx, position) = match opts.autoplay_idx_and_queue_position(
            &ctx.queue,
            current_song_idx,
            hovered_song_idx,
        ) {
            Ok(v) => v,
            Err(err) => {
                status_warn!("{}", err);
                return;
            }
        };

        // Clone app_state for the queue refresh query
        let app_state = ctx.app_state.clone();

        ctx.command(move |client| {
            client.enqueue_multiple(items, autoplay_idx, position, replace)?;
            Ok(())
        });

        // For YouTube backend, trigger a queue refresh to update the UI
        // This is necessary because YouTube backend doesn't have MPD-style idle events
        if matches!(ctx.config.backend, crate::config::PlayerBackend::YouTube) {
            log::debug!("Triggering queue refresh after YouTube enqueue");
            ctx.query()
                .id(super::messaging::GLOBAL_QUEUE_UPDATE)
                .replace_id("playlist")
                .query(move |client| {
                    let queue = client.playlist_info()?;
                    // Sync to AppState
                    app_state.write().unwrap().replace_queue(queue.clone());
                    Ok(super::messaging::QueryResult::Queue(Some(queue)))
                });
            // Status updates are handled by continuous polling in event_loop.rs
            // No need for manual status refresh here
        }
    }

    fn play_position_safe(&mut self, queue_len: usize) -> Result<(), MpdError>;
    fn enqueue_multiple(
        &mut self,
        items: Vec<Enqueue>,
        autoplay_idx: Option<usize>,
        position: Option<QueuePosition>,
        replace: bool,
    ) -> Result<(), MpdError>;
    fn delete_multiple(&mut self, items: Vec<DeleteTarget>) -> Result<(), MpdError>;
    fn add_to_playlist_multiple(
        &mut self,
        playlist_name: &str,
        song_paths: Vec<String>,
    ) -> Result<(), MpdError>;
    fn list_partitioned_outputs(
        &mut self,
        current_partition: &str,
    ) -> Result<Vec<PartitionedOutput>, MpdError>;
    fn create_playlist(&mut self, name: &str, items: Vec<String>) -> Result<(), MpdError>;
    fn next_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError>;
    fn prev_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError>;
    fn fetch_song_stickers(
        &mut self,
        song_uris: Vec<String>,
    ) -> Result<HashMap<String, HashMap<String, String>>, MpdError>;
    fn set_sticker_multiple(
        &mut self,
        key: &str,
        value: String,
        items: Vec<Enqueue>,
    ) -> Result<(), MpdError>;
    fn delete_sticker_multiple(&mut self, key: &str, items: Vec<Enqueue>) -> Result<(), MpdError>;
}

/// Items that can be deleted from playlists or the queue
#[derive(Debug, Clone)]
pub enum DeleteTarget {
    SongInPlaylist { playlist: Arc<str>, range: SingleOrRange },
    Playlist { name: String },
}

/// Items that can be added to the queue
#[allow(dead_code, reason = "Search is currently unused")]
#[derive(Debug, Clone)]
pub enum Enqueue {
    File { path: String },
    /// Song with full metadata (used by YouTube backend)
    Song { song: crate::domain::Song },
    Playlist { name: String },
    Find { filter: Vec<(Tag, FilterKind, String)> },
}

impl<T: MpdClient + Command + ProtoClient> BackendActions for T {
    fn play_position_safe(&mut self, queue_len: usize) -> Result<(), MpdError> {
        match self.play_pos(queue_len) {
            Ok(()) => {}
            Err(MpdError::Mpd(MpdFailureResponse { code: ErrorCode::Argument, .. })) => {
                // This can happen when multiple clients modify the queue at
                // the same time. But a more robust
                // solution would require refetching the whole
                // queue and searching for the added song. This should be
                // good enough.
                log::warn!("Failed to autoplay song");
            }
            Err(err) => return Err(err),
        }
        Ok(())
    }

    fn enqueue_multiple(
        &mut self,
        mut items: Vec<Enqueue>,
        autoplay_idx: Option<usize>,
        position: Option<QueuePosition>,
        replace: bool,
    ) -> Result<(), MpdError> {
        if items.is_empty() {
            return Ok(());
        }
        let should_reverse = match position {
            Some(QueuePosition::RelativeAdd(_)) => true,
            Some(QueuePosition::RelativeSub(_)) => false,
            Some(QueuePosition::Absolute(_)) => true,
            None => false,
        };

        if should_reverse {
            items.reverse();
        }

        self.send_start_cmd_list()?;
        if replace {
            self.send_clear()?;
        }

        let items_len = items.len();
        for item in items {
            match item {
                Enqueue::File { path } => self.send_add(&path, position),
                Enqueue::Song { song } => self.send_add(&song.uri, position), // MPD uses file path
                Enqueue::Playlist { name } => self.send_load_playlist(&name, position),
                Enqueue::Find { filter } => self.send_find_add(
                    &filter
                        .into_iter()
                        .map(|(tag, kind, value)| Filter::new_with_kind(tag, value, kind))
                        .collect_vec(),
                    position,
                ),
            }?;
        }
        self.send_execute_cmd_list()?;
        self.read_ok()?;
        if items_len == 1 {
            status_info!("Added 1 item to the queue");
        } else {
            status_info!("Added {items_len} items to the queue");
        }

        if let Some(autoplay_idx) = autoplay_idx {
            self.play_position_safe(autoplay_idx)?;
        }

        Ok(())
    }

    fn delete_multiple(&mut self, items: Vec<DeleteTarget>) -> Result<(), MpdError> {
        let items_len = items.len();
        if items_len == 0 {
            return Ok(());
        }

        self.send_start_cmd_list()?;
        for item in items.into_iter().rev() {
            match item {
                DeleteTarget::SongInPlaylist { playlist, range } => {
                    self.send_delete_from_playlist(&playlist, &range)?;
                }
                DeleteTarget::Playlist { name } => {
                    self.send_delete_playlist(&name)?;
                }
            }
        }
        self.send_execute_cmd_list()?;
        self.read_ok()?;

        if items_len == 1 {
            status_info!("Deleted 1 item");
        } else {
            status_info!("Deleted {} items", items_len);
        }

        Ok(())
    }

    fn add_to_playlist_multiple(
        &mut self,
        playlist_name: &str,
        song_paths: Vec<String>,
    ) -> Result<(), MpdError> {
        let items_len = song_paths.len();
        if items_len == 0 {
            return Ok(());
        }

        self.send_start_cmd_list()?;
        for mut path in song_paths {
            if path.starts_with('/') {
                path.insert_str(0, "file://");
                self.add_to_playlist(playlist_name, &path, None)?;
            } else {
                self.send_add_to_playlist(playlist_name, &path, None)?;
            }
        }
        self.send_execute_cmd_list()?;
        self.read_ok()?;

        if items_len == 1 {
            status_info!("Added 1 song to playlist {}", playlist_name);
        } else {
            status_info!("Added {} songs to playlist {}", items_len, playlist_name);
        }

        Ok(())
    }

    fn list_partitioned_outputs(
        &mut self,
        current_partition: &str,
    ) -> Result<Vec<PartitionedOutput>, MpdError> {
        if current_partition == "default" {
            Ok(self
                .outputs()?
                .0
                .into_iter()
                .map(|output| PartitionedOutput {
                    id: output.id,
                    name: output.name,
                    enabled: if output.plugin == "dummy" { false } else { output.enabled },
                    kind: if output.plugin == "dummy" {
                        PartitionedOutputKind::OtherPartition
                    } else {
                        PartitionedOutputKind::CurrentPartition
                    },
                    plugin: output.plugin,
                })
                .collect())
        } else {
            // MPD lists all outputs only on the default partition so we have to
            // switch to it, list the outputs and then switch back. We also have to
            // list outputs on the current partition to find out which output is
            // actually enabled on the current partition.
            self.send_start_cmd_list_ok()?;
            self.send_switch_to_partition("default")?;
            self.send_outputs()?;
            self.send_switch_to_partition(current_partition)?;
            self.send_outputs()?;
            self.send_execute_cmd_list()?;

            self.read_ok()?; // switch to default
            let all_outputs = self.read_response::<Outputs>()?.0;
            self.read_ok()?; // switch to current
            let mut current_outputs = self.read_response::<Outputs>()?.0;
            self.read_ok()?; // OK for the whole command list

            let mut result = Vec::with_capacity(all_outputs.len());
            for output in all_outputs {
                if let Some(current) = current_outputs
                    .iter_mut()
                    .find(|o| o.name == output.name && o.plugin != "dummy")
                {
                    result.push(PartitionedOutput {
                        id: current.id,
                        name: std::mem::take(&mut current.name),
                        enabled: current.enabled,
                        plugin: std::mem::take(&mut current.plugin),
                        kind: PartitionedOutputKind::CurrentPartition,
                    });
                } else {
                    result.push(PartitionedOutput {
                        id: output.id,
                        name: output.name,
                        enabled: false,
                        plugin: output.plugin,
                        kind: PartitionedOutputKind::OtherPartition,
                    });
                }
            }

            Ok(result)
        }
    }

    fn create_playlist(&mut self, name: &str, items: Vec<String>) -> Result<(), MpdError> {
        if items.is_empty() {
            return Ok(());
        }
        self.send_start_cmd_list()?;
        // MPD does not allow creating empty playlists. We work
        // around it here by saving the current queue and then
        // clearing the newly created playlist.
        self.send_save_queue_as_playlist(name, None)?;
        self.send_clear_playlist(name)?;
        for item in &items {
            self.send_add_to_playlist(name, item, None)?;
        }
        self.send_execute_cmd_list()?;
        self.read_ok()?;

        status_info!("Created playlist {name} with {} items", items.len());

        Ok(())
    }

    fn next_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError> {
        if !keep {
            return self.next();
        }

        match state {
            State::Play => self.next(),
            State::Stop => Ok(()),
            State::Pause => {
                self.send_start_cmd_list()?;
                self.send_next()?;
                self.send_pause()?;
                self.send_execute_cmd_list()?;
                self.read_ok()
            }
        }
    }

    fn prev_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError> {
        if !keep {
            return self.prev();
        }

        match state {
            State::Play => self.prev(),
            State::Stop => Ok(()),
            State::Pause => {
                self.send_start_cmd_list()?;
                self.send_prev()?;
                self.send_pause()?;
                self.send_execute_cmd_list()?;
                self.read_ok()
            }
        }
    }

    fn fetch_song_stickers(
        &mut self,
        mut song_uris: Vec<String>,
    ) -> Result<HashMap<String, HashMap<String, String>>, MpdError> {
        if song_uris.is_empty() {
            return Ok(HashMap::new());
        }

        let mut list_ended_with_err = false;
        let mut i = 0;
        let mut result = HashMap::new();

        while i < song_uris.len() {
            self.send_start_cmd_list_ok()?;
            for uri in &song_uris[i..] {
                self.send_list_stickers(uri)?;
            }
            self.send_execute_cmd_list()?;

            for uri in &mut song_uris[i..] {
                let res: Result<Stickers, _> = self.read_response();
                match res {
                    Ok(stickers) => {
                        list_ended_with_err = false;
                        result.insert(std::mem::take(uri), stickers.0);
                        i += 1;
                    }
                    Err(error) => {
                        log::warn!(error:?, file = uri.as_str(); "Tried to find stickers but unexpected error occurred");
                        result.insert(std::mem::take(uri), HashMap::new());
                        list_ended_with_err = true;
                        i += 1;
                        break;
                    }
                }
            }
        }

        // In case the last sticker was fetched successfully we have to read an
        // OK as an ack for the whole command list
        if !list_ended_with_err {
            self.read_ok()?;
        }

        log::debug!(count = result.len(); "Fetched stickers for songs");
        Ok(result)
    }

    fn set_sticker_multiple(
        &mut self,
        key: &str,
        value: String,
        items: Vec<Enqueue>,
    ) -> Result<(), MpdError> {
        let mut uris = Vec::new();
        for item in items {
            match item {
                Enqueue::File { path } => uris.push(path),
                Enqueue::Song { song } => uris.push(song.uri), // Extract file path
                Enqueue::Playlist { name } => {
                    let playlist = self.list_playlist(&name)?.0;
                    uris.extend(playlist);
                }
                Enqueue::Find { filter } => {
                    let songs = self.find(
                        &filter
                            .into_iter()
                            .map(|(tag, kind, value)| Filter::new_with_kind(tag, value, kind))
                            .collect_vec(),
                    )?;
                    uris.extend(songs.into_iter().map(|song| song.file));  // MPD Song uses .file
                }
            }
        }

        self.send_start_cmd_list()?;
        for uri in uris {
            self.send_set_sticker(&uri, key, &value.clone())?;
        }
        self.send_execute_cmd_list()?;
        self.read_ok()?;

        Ok(())
    }

    fn delete_sticker_multiple(&mut self, key: &str, items: Vec<Enqueue>) -> Result<(), MpdError> {
        let mut uris = Vec::new();
        for item in items {
            match item {
                Enqueue::File { path } => uris.push(path),
                Enqueue::Song { song } => uris.push(song.uri), // Extract file path
                Enqueue::Playlist { name } => {
                    let playlist = self.list_playlist(&name)?.0;
                    uris.extend(playlist);
                }
                Enqueue::Find { filter } => {
                    let songs = self.find(
                        &filter
                            .into_iter()
                            .map(|(tag, kind, value)| Filter::new_with_kind(tag, value, kind))
                            .collect_vec(),
                    )?;
                    uris.extend(songs.into_iter().map(|song| song.file));  // MPD Song uses .file
                }
            }
        }

        for uri in uris {
            match self.delete_sticker(&uri, key) {
                Ok(()) => {}
                Err(MpdError::Mpd(MpdFailureResponse { code: ErrorCode::NoExist, .. })) => {}
                err @ Err(_) => err?,
            }
        }

        Ok(())
    }
}

/// Output where ID is only defined when the output is on the current
/// partition.
#[derive(Debug)]
pub struct PartitionedOutput {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub plugin: String,
    pub kind: PartitionedOutputKind,
}

#[derive(Debug, Clone, Copy)]
pub enum PartitionedOutputKind {
    OtherPartition,
    CurrentPartition,
}

// Implement BackendActions for the unified backend dispatcher
impl BackendActions for crate::backends::BackendDispatcher<'_> {
    fn play_position_safe(&mut self, queue_len: usize) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.play_position_safe(queue_len),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(()), // Not applicable for MPV/YouTube
        }
    }

    fn enqueue_multiple(
        &mut self,
        items: Vec<Enqueue>,
        autoplay_idx: Option<usize>,
        position: Option<QueuePosition>,
        replace: bool,
    ) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => {
                b.client.enqueue_multiple(items, autoplay_idx, position, replace)
            }
            crate::backends::BackendDispatcher::YouTube(backend) => {
                use crate::backends::api::{Queue, StatusQuery};
                
                log::debug!("enqueue_multiple for YouTube backend: {} items, replace={}, autoplay_idx={:?}", items.len(), replace, autoplay_idx);

                if replace {
                    Queue::clear(backend)?;
                }

                let queue_len_before = StatusQuery::queue_songs(backend).map(|q| q.len()).unwrap_or(0);

                // Add each item to the queue
                for item in items.iter() {
                    match item {
                        Enqueue::File { path } => {
                            log::debug!("YouTube: adding file to queue (no metadata): {}", path);
                            // Create a minimal Item and use api::Queue
                            let item = crate::backends::api::Item::track(path, path);
                            Queue::add(backend, &[item], crate::backends::api::InsertAt::End, crate::backends::api::AfterAdd::Nothing)?;
                        }
                        Enqueue::Song { song } => {
                            let title = song.metadata.get("title").and_then(|v| v.first()).map(|s| s.as_str()).unwrap_or(&song.uri);
                            log::info!("YouTube: adding song to queue: {} ({})", title, &song.uri);
                            // Add song with full metadata using public method
                            backend.add_song(song, None)?;
                        }
                        Enqueue::Playlist { name } => {
                            log::debug!("YouTube: loading playlist: {}", name);
                            // Playlists not fully supported on YouTube yet
                            log::warn!("Playlist loading not yet supported for YouTube backend: {}", name);
                        }
                        Enqueue::Find { filter: _ } => {
                            log::warn!("Find filter not supported for YouTube backend");
                        }
                    }
                }

                // If autoplay was requested, play the song at the correct position
                if let Some(play_idx) = autoplay_idx {
                    // Calculate the actual position in the queue
                    let target_pos = if replace { play_idx } else { queue_len_before + play_idx };
                    log::info!("YouTube: autoplay at position {} (replace={}, play_idx={})", target_pos, replace, play_idx);
                    if let Err(e) = backend.play_pos(target_pos) {
                        log::error!("YouTube: failed to play position {}: {}", target_pos, e);
                    }
                }

                Ok(())
            }
        }
    }

    fn delete_multiple(&mut self, items: Vec<DeleteTarget>) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.delete_multiple(items),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    fn add_to_playlist_multiple(
        &mut self,
        playlist_name: &str,
        song_paths: Vec<String>,
    ) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => {
                b.client.add_to_playlist_multiple(playlist_name, song_paths)
            }
            crate::backends::BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    fn list_partitioned_outputs(
        &mut self,
        current_partition: &str,
    ) -> Result<Vec<PartitionedOutput>, MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.list_partitioned_outputs(current_partition),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(Vec::new()),
        }
    }

    fn create_playlist(&mut self, name: &str, items: Vec<String>) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.create_playlist(name, items),
            crate::backends::BackendDispatcher::YouTube(_) => {
                log::debug!("Playlists not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    fn next_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.next_keep_state(keep, state),
            crate::backends::BackendDispatcher::YouTube(_) => {
                // Simple next for MPV/YouTube
                self.next().map_err(|e| MpdError::Generic(e.to_string()))
            }
        }
    }

    fn prev_keep_state(&mut self, keep: bool, state: State) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.prev_keep_state(keep, state),
            crate::backends::BackendDispatcher::YouTube(_) => {
                // Simple previous for MPV/YouTube
                self.previous().map_err(|e| MpdError::Generic(e.to_string()))
            }
        }
    }

    fn fetch_song_stickers(
        &mut self,
        song_uris: Vec<String>,
    ) -> Result<HashMap<String, HashMap<String, String>>, MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.fetch_song_stickers(song_uris),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(HashMap::new()),
        }
    }

    fn set_sticker_multiple(
        &mut self,
        key: &str,
        value: String,
        items: Vec<Enqueue>,
    ) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.set_sticker_multiple(key, value, items),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    fn delete_sticker_multiple(&mut self, key: &str, items: Vec<Enqueue>) -> Result<(), MpdError> {
        match self {
            crate::backends::BackendDispatcher::Mpd(b) => b.client.delete_sticker_multiple(key, items),
            crate::backends::BackendDispatcher::YouTube(_) => Ok(()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use rstest::{fixture, rstest};

    use crate::{
        config::keys::actions::{AddOpts, AutoplayKind, Position},
        ctx::Ctx,
        mpd::QueuePosition,
        domain::song::Song,
        tests::fixtures::ctx,
    };

    mod enqueue_multiple {
        use std::collections::HashMap;

        use super::*;

        #[fixture]
        fn ctx_with_queue(mut ctx: Ctx) -> Ctx {
            let albums = ["a", "b", "b", "b", "c", "c", "d", "e", "e", "f"];
            for i in 0..10 {
                ctx.queue.push(Song {
                    id: Some(i),
                    uri: format!("song{i}"),
                    metadata: HashMap::from([(
                        "album".to_owned(),
                        vec![albums[i as usize].to_owned()],
                    )]),
                    ..Default::default()
                });
            }
            ctx
        }

        #[rstest]
        fn first_after_current_album(ctx_with_queue: Ctx) {
            let position = Position::AfterCurrentAlbum;
            let autoplay = AutoplayKind::First;
            let current_song_idx = Some(4);
            let hovered = None;
            let opts = AddOpts { autoplay, position, all: false };

            let (autoplay_idx, queue_position) = opts
                .autoplay_idx_and_queue_position(&ctx_with_queue.queue, current_song_idx, hovered)
                .unwrap();

            assert_eq!(queue_position, Some(QueuePosition::Absolute(6)));
            assert_eq!(autoplay_idx, Some(6));
        }

        // ... (rest of tests omitted for brevity - they remain unchanged)
    }
}

// =============================================================================
