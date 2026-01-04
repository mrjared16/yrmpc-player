//! Optional trait implementations for MpdBackend.
//!
//! This module implements the MPD-specific optional traits defined in
//! `optional.rs`.

use std::collections::HashMap;

use anyhow::Result;

use super::{
    MpdBackend,
    optional::{Database, Modes, Outputs, SavedPlaylists, Stickers},
};
use crate::{
    domain::{QueuePosition, Song},
    mpd::{
        SingleOrRange,
        commands::{OnOffOneshot, Output, Playlist, SaveMode},
        mpd_client::MpdClient as MpdClientTrait,
    },
};

// =============================================================================
// STICKERS
// =============================================================================

impl<'name> Stickers for MpdBackend<'name> {
    fn list(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.client.list_stickers(uri).map(|s| s.0).map_err(Into::into)
    }

    fn set(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.client.set_sticker(uri, key, value).map_err(Into::into)
    }

    fn delete(&mut self, uri: &str, key: &str) -> Result<()> {
        self.client.delete_sticker(uri, key).map_err(Into::into)
    }
}

// =============================================================================
// SAVED PLAYLISTS
// =============================================================================

impl<'name> SavedPlaylists for MpdBackend<'name> {
    fn list(&mut self) -> Result<Vec<Playlist>> {
        self.client.list_playlists().map_err(Into::into)
    }

    fn get(&mut self, name: &str) -> Result<Vec<Song>> {
        self.client
            .list_playlist_info(name, None)
            .map(|songs| songs.into_iter().map(|s| s.into()).collect())
            .map_err(Into::into)
    }

    fn load(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()> {
        let mpd_pos = position.map(|p| match p {
            QueuePosition::Absolute(i) => crate::mpd::QueuePosition::Absolute(i),
            QueuePosition::Relative(i) if i >= 0 => {
                crate::mpd::QueuePosition::RelativeAdd(i as usize)
            }
            QueuePosition::Relative(i) => crate::mpd::QueuePosition::RelativeSub((-i) as usize),
            QueuePosition::End => crate::mpd::QueuePosition::Absolute(usize::MAX),
            QueuePosition::Next => crate::mpd::QueuePosition::RelativeAdd(1),
        });
        self.client.load_playlist(name, mpd_pos).map_err(Into::into)
    }

    fn save(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()> {
        self.client.save_queue_as_playlist(name, mode).map_err(Into::into)
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        self.client.delete_playlist(name).map_err(Into::into)
    }

    fn rename(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.client.rename_playlist(old_name, new_name).map_err(Into::into)
    }

    fn add_song(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.client.add_to_playlist(playlist, uri, None).map_err(Into::into)
    }

    fn remove_song(&mut self, playlist: &str, position: u32) -> Result<()> {
        use crate::mpd::SingleOrRange;
        self.client
            .delete_from_playlist(playlist, &SingleOrRange::single(position as usize))
            .map_err(Into::into)
    }

    fn move_songs(&mut self, playlist: &str, from: SingleOrRange, to: u32) -> Result<()> {
        self.client.move_in_playlist(playlist, &from, to as usize).map_err(Into::into)
    }
}

// =============================================================================
// OUTPUTS
// =============================================================================

impl<'name> Outputs for MpdBackend<'name> {
    fn list(&mut self) -> Result<Vec<Output>> {
        self.client.outputs().map(|o| o.0).map_err(Into::into)
    }

    fn enable(&mut self, id: u32) -> Result<()> {
        self.client.enable_output(id).map_err(Into::into)
    }

    fn disable(&mut self, id: u32) -> Result<()> {
        self.client.disable_output(id).map_err(Into::into)
    }

    fn toggle(&mut self, id: u32) -> Result<()> {
        self.client.toggle_output(id).map_err(Into::into)
    }
}

// =============================================================================
// DATABASE
// =============================================================================

impl<'name> Database for MpdBackend<'name> {
    fn update(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.update(path).map(|u| u.job_id).map_err(Into::into)
    }

    fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.rescan(path).map(|u| u.job_id).map_err(Into::into)
    }
}

// =============================================================================
// MODES
// =============================================================================

impl<'name> Modes for MpdBackend<'name> {
    fn set_single(&mut self, mode: OnOffOneshot) -> Result<()> {
        self.client.single(mode).map_err(Into::into)
    }

    fn set_consume(&mut self, mode: OnOffOneshot) -> Result<()> {
        self.client.consume(mode).map_err(Into::into)
    }

    fn set_crossfade(&mut self, seconds: u32) -> Result<()> {
        self.client.crossfade(seconds).map_err(Into::into)
    }

    fn shuffle_range(&mut self, range: Option<SingleOrRange>) -> Result<()> {
        self.client.shuffle(range).map_err(Into::into)
    }
}
