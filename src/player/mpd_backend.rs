use std::{borrow::Cow, collections::HashMap};

use anyhow::Result;

use super::backend::MusicBackend;
use crate::mpd::{
    client::Client as MpdClient,
    commands::{
        Count,
        Decoder,
        LsInfoEntry,
        OnOffOneshot,
        Output,
        Playlist,
        QueuePosition,
        SaveMode,
        SeekPosition,
        Song,
        Status,
        Tag,
        ValueChange,
        Volume,
        lsinfo::Dir,
    },
    mpd_client::{Filter, FilterKind, MpdClient as MpdClientTrait, SingleOrRange},
    version::Version,
};

/// MPD backend implementation
///
/// This wraps the existing MPD client and implements the MusicBackend trait.
/// All methods delegate to the existing MPD implementation.
#[derive(Debug)]
pub struct MpdBackend<'name> {
    pub client: MpdClient<'name>,
}

impl<'name> MpdBackend<'name> {
    pub fn new(client: MpdClient<'name>) -> Self {
        Self { client }
    }

    /// Get mutable reference to inner MPD client for MPD-specific operations
    pub fn client_mut(&mut self) -> &mut MpdClient<'name> {
        &mut self.client
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
        self.client.prev().map_err(Into::into)
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
        self.client.get_status().map_err(Into::into)
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        Ok(self
            .client
            .playlist_info()
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .unwrap_or_default())
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        self.client.get_current_song().map_err(Into::into)
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
        self.client.move_id(from, QueuePosition::Absolute(to as usize)).map_err(Into::into)
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        self.client.play_id(id).map_err(Into::into)
    }

    // ===== Volume Control =====

    fn volume(&mut self) -> Result<u8> {
        // Get current volume from status
        let status = self
            .client
            .get_status()
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?;
        Ok(status.volume.0 as u8)
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        // Convert ValueChange to Volume for MPD
        match volume {
            ValueChange::Set(v) => self.client.set_volume(Volume(v as u32)).map_err(Into::into),
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

    fn shuffle(&mut self, range: Option<SingleOrRange>) -> Result<()> {
        self.client.shuffle(range).map_err(Into::into)
    }

    // ===== Library Browsing =====

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(self
            .client
            .lsinfo(path)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .0)
    }

    fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        use crate::mpd::commands::list_all::ListAllEntry;
        let list_all = self
            .client
            .list_all(path)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?;
        Ok(list_all
            .0
            .into_iter()
            .map(|entry| match entry {
                ListAllEntry::File(path) => {
                    LsInfoEntry::File(Song { file: path, ..Default::default() })
                }
                ListAllEntry::Dir(path) => LsInfoEntry::Dir(Dir {
                    full_path: path.clone(),
                    name: path.split('/').last().unwrap_or("").to_string(),
                    ..Default::default()
                }),
                ListAllEntry::Playlist(path) => {
                    LsInfoEntry::Playlist(crate::mpd::commands::lsinfo::Playlist {
                        full_path: path.clone(),
                        name: path.split('/').last().unwrap_or("").to_string(),
                        ..Default::default()
                    })
                }
            })
            .collect())
    }

    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>> {
        self.client.search(filter, false).map_err(Into::into)
    }

    fn find(&mut self, filter: &[(Tag, String)], _window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        // MPD client find() doesn't support window directly in this version wrapper
        // TODO: Implement window support if critical
        let filters = convert_filter(filter);
        self.client.find(&filters).map_err(Into::into)
    }

    fn list_tag(&mut self, tag: Tag, filter: Option<&[(Tag, String)]>) -> Result<Vec<String>> {
        let filters = filter.map(convert_filter);
        let filters_ref = filters.as_deref();
        // list_tag returns MpdList which wraps Vec<String>
        Ok(self.client.list_tag(tag, filters_ref).map_err(|e| anyhow::Error::from(e))?.0)
    }

    fn count(&mut self, filter: &[(Tag, String)]) -> Result<(usize, std::time::Duration)> {
        let filters = convert_filter(filter);
        let count = self
            .client
            .count(&filters)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?;
        Ok((count.songs, count.playtime))
    }

    // ===== Playlist Management =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        self.client.list_playlists().map_err(Into::into)
    }

    fn playlist_info_name(&mut self, name: &str) -> Result<Vec<Song>> {
        self.client.list_playlist_info(name, None).map_err(Into::into)
    }

    fn load_playlist(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()> {
        self.client.load_playlist(name, position).map_err(Into::into)
    }

    fn save_queue_as_playlist(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()> {
        self.client.save_queue_as_playlist(name, mode).map_err(Into::into)
    }

    fn delete_playlist(&mut self, name: &str) -> Result<()> {
        self.client.delete_playlist(name).map_err(Into::into)
    }

    fn rename_playlist(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.client.rename_playlist(old_name, new_name).map_err(Into::into)
    }

    fn add_to_playlist(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.client.add_to_playlist(playlist, uri, None).map_err(Into::into)
    }

    fn delete_from_playlist(&mut self, playlist: &str, position: u32) -> Result<()> {
        self.client
            .delete_from_playlist(playlist, &SingleOrRange::single(position as usize))
            .map_err(Into::into)
    }

    fn move_in_playlist(&mut self, playlist: &str, from: u32, to: u32) -> Result<()> {
        self.client
            .move_in_playlist(playlist, &SingleOrRange::single(from as usize), to as usize)
            .map_err(Into::into)
    }

    // ===== Sticker Support =====

    fn list_stickers(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        Ok(self
            .client
            .list_stickers(uri)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .into_iter()
            .collect())
    }

    fn set_sticker(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.client.set_sticker(uri, key, value).map_err(Into::into)
    }

    fn delete_sticker(&mut self, uri: &str, key: &str) -> Result<()> {
        self.client.delete_sticker(uri, key).map_err(Into::into)
    }

    // ===== Database Management =====

    fn update(&mut self, path: Option<&str>) -> Result<u32> {
        Ok(self
            .client
            .update(path)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .job_id)
    }

    fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        Ok(self
            .client
            .rescan(path)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .job_id)
    }

    // ===== System Info =====

    fn version(&self) -> crate::mpd::version::Version {
        self.client.version
    }

    fn outputs(&mut self) -> Result<Vec<Output>> {
        Ok(self
            .client
            .outputs()
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .0)
    }

    fn decoders(&mut self) -> Result<Vec<Decoder>> {
        Ok(self
            .client
            .decoders()
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .0)
    }

    fn partitions(&mut self) -> Result<Vec<String>> {
        // MPD partitions support - stub for now as it's not in MpdClient trait
        Ok(vec![])
    }

    // ===== Backend Identification =====

    fn backend_name(&self) -> &'static str {
        "MPD"
    }

    fn supports_command(&self, command: &str) -> bool {
        // Check against supported commands if available
        // For now, just return true as MPD supports most things
        true
    }
}

fn convert_filter(filter: &[(Tag, String)]) -> Vec<Filter<'_>> {
    filter
        .iter()
        .map(|(tag, value)| Filter {
            tag: tag.clone(),
            value: Cow::Borrowed(value),
            kind: FilterKind::Contains, // Default to contains
        })
        .collect()
}
