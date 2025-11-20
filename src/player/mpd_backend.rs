use std::collections::HashMap;

use anyhow::Result;

use super::backend::MusicBackend;
use crate::mpd::{
    client::Client as MpdClient,
    commands::*,
    mpd_client::MpdClient as MpdClientTrait,
};

/// MPD backend implementation
///
/// This wraps the existing MPD client and implements the MusicBackend trait.
/// All methods delegate to the existing MPD implementation.
pub struct MpdBackend<'name> {
    pub client: MpdClient<'name>,
}

impl<'name> MpdBackend<'name> {
    pub fn new(client: MpdClient<'name>) -> Self {
        Self { client }
    }

    pub fn into_inner(self) -> MpdClient<'name> {
        self.client
    }

    pub fn inner(&self) -> &MpdClient<'name> {
        &self.client
    }

    pub fn inner_mut(&mut self) -> &mut MpdClient<'name> {
        &mut self.client
    }
}

impl<'name> MusicBackend for MpdBackend<'name> {
    // ===== Playback Control =====

    fn play(&mut self) -> Result<()> {
        self.client.play().map_err(Into::into)
    }

    fn pause(&mut self, state: bool) -> Result<()> {
        // MPD's pause() toggles, so we need to check current state first
        // For now, just call pause() which toggles
        // TODO: Check current state and only call if needed
        self.client.pause().map_err(Into::into)
    }

    fn stop(&mut self) -> Result<()> {
        self.client.stop().map_err(Into::into)
    }

    fn next(&mut self) -> Result<()> {
        self.client.next().map_err(Into::into)
    }

    fn previous(&mut self) -> Result<()> {
        self.client.previous().map_err(Into::into)
    }

    fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        // Convert SeekPosition to ValueChange for MPD
        let value_change = match position {
            SeekPosition::Absolute(secs) => ValueChange::Set(secs as u32),
            SeekPosition::Relative(secs) => {
                if secs >= 0.0 {
                    ValueChange::Increase(secs as u32)
                } else {
                    ValueChange::Decrease((-secs) as u32)
                }
            }
        };
        self.client.seek_current(value_change).map_err(Into::into)
    }

    // ===== Status Queries =====

    fn get_status(&mut self) -> Result<Status> {
        self.client.status().map_err(Into::into)
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        self.client.playlist_info().map_err(Into::into)
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        self.client.current_song().map_err(Into::into)
    }

    // ===== Queue Management =====

    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        self.client.add(uri, position).map_err(Into::into)
    }

    fn delete_id(&mut self, id: u32) -> Result<()> {
        self.client.delete_id(id).map_err(Into::into)
    }

    fn clear(&mut self) -> Result<()> {
        self.client.clear().map_err(Into::into)
    }

    fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.client.move_id(from, to).map_err(Into::into)
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        self.client.play_id(id).map_err(Into::into)
    }

    // ===== Volume Control =====

    fn volume(&mut self) -> Result<u8> {
        // Get current volume from status
        let status = self.client.status().map_err(Into::into)?;
        Ok(status.volume)
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        // Convert ValueChange to Volume for MPD
        match volume {
            ValueChange::Set(v) => self.client.set_volume(Volume::new(v as u8)).map_err(Into::into),
            ValueChange::Increase(delta) | ValueChange::Decrease(delta) => {
                self.client.volume(volume).map_err(Into::into)
            }
        }
    }

    // ===== Playback Options =====

    fn repeat(&mut self, repeat: bool) -> Result<()> {
        self.client.repeat(repeat).map_err(Into::into)
    }

    fn random(&mut self, random: bool) -> Result<()> {
        self.client.random(random).map_err(Into::into)
    }

    fn single(&mut self, single: OnOffOneshot) -> Result<()> {
        self.client.single(single).map_err(Into::into)
    }

    fn consume(&mut self, consume: OnOffOneshot) -> Result<()> {
        self.client.consume(consume).map_err(Into::into)
    }

    fn crossfade(&mut self, seconds: u32) -> Result<()> {
        self.client.crossfade(seconds).map_err(Into::into)
    }

    // ===== Library Browsing =====

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.client.lsinfo(path).map_err(Into::into)
    }

    fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.client.list_all(path).map_err(Into::into)
    }

    fn search(&mut self, filter: &[(Tag, String)]) -> Result<Vec<Song>> {
        self.client.search(filter).map_err(Into::into)
    }

    fn find(&mut self, filter: &[(Tag, String)], window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        self.client.find(filter, window).map_err(Into::into)
    }

    fn list_tag(&mut self, tag: Tag, filter: Option<&[(Tag, String)]>) -> Result<Vec<String>> {
        self.client.list_tag(tag, filter).map_err(Into::into)
    }

    fn count(&mut self, filter: &[(Tag, String)]) -> Result<(usize, std::time::Duration)> {
        self.client.count(filter).map_err(Into::into)
    }

    // ===== Playlist Management =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        self.client.list_playlists().map_err(Into::into)
    }

    fn playlist_info_name(&mut self, name: &str) -> Result<Vec<Song>> {
        self.client.playlist_info_name(name).map_err(Into::into)
    }

    fn load_playlist(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()> {
        self.client.load_playlist(name, position).map_err(Into::into)
    }

    fn save_queue_as_playlist(&mut self, name: &str) -> Result<()> {
        self.client.save_queue_as_playlist(name).map_err(Into::into)
    }

    fn delete_playlist(&mut self, name: &str) -> Result<()> {
        self.client.delete_playlist(name).map_err(Into::into)
    }

    fn rename_playlist(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.client.rename_playlist(old_name, new_name).map_err(Into::into)
    }

    fn add_to_playlist(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.client.add_to_playlist(playlist, uri).map_err(Into::into)
    }

    fn delete_from_playlist(&mut self, playlist: &str, position: u32) -> Result<()> {
        self.client.delete_from_playlist(playlist, position).map_err(Into::into)
    }

    fn move_in_playlist(&mut self, playlist: &str, from: u32, to: u32) -> Result<()> {
        self.client.move_in_playlist(playlist, from, to).map_err(Into::into)
    }

    // ===== Sticker Support =====

    fn list_stickers(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.client.list_stickers(uri).map_err(Into::into)
    }

    fn set_sticker(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.client.set_sticker(uri, key, value).map_err(Into::into)
    }

    fn delete_sticker(&mut self, uri: &str, key: &str) -> Result<()> {
        self.client.delete_sticker(uri, key).map_err(Into::into)
    }

    // ===== Database Management =====

    fn update(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.update(path).map_err(Into::into)
    }

    fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.rescan(path).map_err(Into::into)
    }

    // ===== System Info =====

    fn version(&self) -> crate::mpd::version::Version {
        self.client.version
    }

    fn outputs(&mut self) -> Result<Vec<Output>> {
        self.client.outputs().map_err(Into::into)
    }

    fn decoders(&mut self) -> Result<Vec<Decoder>> {
        self.client.decoders().map_err(Into::into)
    }

    fn partitions(&mut self) -> Result<Vec<String>> {
        self.client.partitions().map_err(Into::into)
    }

    // ===== Backend Identification =====

    fn backend_name(&self) -> &'static str {
        "MPD"
    }

    fn supports_command(&self, command: &str) -> bool {
        self.client.supported_commands.contains(command)
    }
}
