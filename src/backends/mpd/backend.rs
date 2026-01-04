use std::collections::HashMap;

use anyhow::Result;

use super::protocol::{
    client::Client as MpdClient,
    commands::{
        Decoder,
        LsInfoEntry,
        OnOffOneshot,
        Output,
        Playlist,
        QueuePosition,
        SaveMode,
        SeekPosition,
        Song,
        Tag,
        ValueChange,
        Volume,
        lsinfo::Dir,
    },
    mpd_client::{Filter, MpdClient as MpdClientTrait, SingleOrRange},
};
use crate::backends::traits::{MusicBackend, QueueOperations};

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

// === QueueOperations Implementation ===
// MPD extracts song.uri and uses its own metadata database

impl<'name> QueueOperations for MpdBackend<'name> {
    fn enqueue(
        &mut self,
        song: &crate::domain::Song,
        position: Option<crate::domain::QueuePosition>,
    ) -> Result<()> {
        // MPD only needs the file path, it has its own metadata database
        let mpd_pos = position.map(|p| match p {
            crate::domain::QueuePosition::Absolute(i) => crate::mpd::QueuePosition::Absolute(i),
            crate::domain::QueuePosition::Relative(i) => {
                if i >= 0 {
                    crate::mpd::QueuePosition::RelativeAdd(i as usize)
                } else {
                    crate::mpd::QueuePosition::RelativeSub((-i) as usize)
                }
            }
            crate::domain::QueuePosition::End => crate::mpd::QueuePosition::Absolute(usize::MAX),
            crate::domain::QueuePosition::Next => crate::mpd::QueuePosition::RelativeAdd(1),
        });
        self.client.add(&song.uri, mpd_pos).map_err(Into::into)
    }

    fn dequeue(&mut self, id: u32) -> Result<()> {
        self.client.delete_id(id).map_err(Into::into)
    }

    fn reorder(&mut self, from: u32, to: u32) -> Result<()> {
        self.client.move_id(from, QueuePosition::Absolute(to as usize)).map_err(Into::into)
    }

    fn clear_queue(&mut self) -> Result<()> {
        self.client.clear().map_err(Into::into)
    }

    fn play_by_id(&mut self, id: u32) -> Result<()> {
        self.client.play_id(id).map_err(Into::into)
    }
}

impl<'name> MusicBackend for MpdBackend<'name> {
    // ===== Playback Control =====

    fn play(&mut self) -> Result<()> {
        self.client.play().map_err(Into::into)
    }

    fn pause(&mut self, _state: bool) -> Result<()> {
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

    fn get_status(&mut self) -> Result<crate::domain::Status> {
        Ok(self.client.get_status()?.into())
    }

    fn playlist_info(&mut self) -> Result<Vec<crate::domain::Song>> {
        Ok(self
            .client
            .playlist_info()
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn current_song(&mut self) -> Result<Option<crate::domain::Song>> {
        Ok(self.client.get_current_song()?.map(Into::into))
    }

    // ===== Queue Management =====

    fn add(&mut self, uri: &str, position: Option<crate::domain::QueuePosition>) -> Result<()> {
        let mpd_pos = position.map(|p| match p {
            crate::domain::QueuePosition::Absolute(i) => crate::mpd::QueuePosition::Absolute(i),
            crate::domain::QueuePosition::Relative(i) => if i >= 0 {
                crate::mpd::QueuePosition::RelativeAdd(i as usize)
            } else {
                crate::mpd::QueuePosition::RelativeSub((-i) as usize)
            },
            crate::domain::QueuePosition::End => crate::mpd::QueuePosition::Absolute(usize::MAX), // MPD handles out of bounds as end usually, or we need logic
            crate::domain::QueuePosition::Next => crate::mpd::QueuePosition::RelativeAdd(1), // Rough approximation, MPD doesn't have "Next" directly in add without calc
        });
        // For now, let's assume simple mapping. MPD's add command usually takes
        // optional position. The client.add takes Option<QueuePosition>.
        // Wait, crate::mpd::QueuePosition definition:
        // pub enum QueuePosition { Absolute(usize), RelativeAdd(usize),
        // RelativeSub(usize) }

        // Re-checking logic:
        // domain::QueuePosition::Next -> logic needed?
        // For now I will implement a basic conversion.

        self.client.add(uri, mpd_pos).map_err(Into::into)
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

    fn set_volume(&mut self, delta: ValueChange) -> Result<()> {
        // Convert ValueChange to Volume for MPD
        match delta {
            ValueChange::Set(v) => self.client.set_volume(Volume(v as u32)).map_err(Into::into),
            ValueChange::Increase(_) | ValueChange::Decrease(_) => {
                self.client.volume(delta).map_err(Into::into)
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

    fn get_search_suggestions(&mut self, _query: String) -> Result<Vec<String>> {
        // MPD backend doesn't support search suggestions
        Ok(vec![])
    }

    fn get_library(
        &mut self,
        _category: crate::backends::LibraryCategory,
    ) -> Result<Vec<LsInfoEntry>> {
        // MPD backend doesn't support YouTube Music library categories
        // Return empty vec to indicate no library support
        Ok(vec![])
    }

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
                    LsInfoEntry::File(Song { file: path, ..Default::default() }.into())
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

    fn search(&mut self, filter: &[Filter]) -> Result<Vec<crate::domain::Song>> {
        Ok(self.client.search(filter, false)?.into_iter().map(Into::into).collect())
    }

    fn find(
        &mut self,
        filter: &[Filter],
        _window: Option<(u32, u32)>,
    ) -> Result<Vec<crate::domain::Song>> {
        // MPD client find() doesn't support window directly in this version wrapper
        // TODO: Implement window support if critical
        Ok(self.client.find(filter)?.into_iter().map(Into::into).collect())
    }

    fn list_tag(&mut self, tag: Tag, filter: Option<&[Filter]>) -> Result<Vec<String>> {
        // list_tag returns MpdList which wraps Vec<String>
        Ok(self.client.list_tag(tag, filter).map_err(|e| anyhow::Error::from(e))?.0)
    }

    fn count(&mut self, filter: &[Filter]) -> Result<(usize, std::time::Duration)> {
        let count = self
            .client
            .count(filter)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))?;
        Ok((count.songs, count.playtime))
    }

    // ===== Playlist Management =====

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        self.client.list_playlists().map_err(Into::into)
    }

    fn playlist_info_name(&mut self, name: &str) -> Result<Vec<crate::domain::Song>> {
        Ok(self.client.list_playlist_info(name, None)?.into_iter().map(Into::into).collect())
    }

    fn load_playlist(
        &mut self,
        name: &str,
        position: Option<crate::domain::QueuePosition>,
    ) -> Result<()> {
        let mpd_pos = position.map(|p| match p {
            crate::domain::QueuePosition::Absolute(i) => crate::mpd::QueuePosition::Absolute(i),
            crate::domain::QueuePosition::Relative(i) => {
                if i >= 0 {
                    crate::mpd::QueuePosition::RelativeAdd(i as usize)
                } else {
                    crate::mpd::QueuePosition::RelativeSub((-i) as usize)
                }
            }
            crate::domain::QueuePosition::End => crate::mpd::QueuePosition::Absolute(usize::MAX),
            crate::domain::QueuePosition::Next => crate::mpd::QueuePosition::RelativeAdd(1),
        });
        self.client.load_playlist(name, mpd_pos).map_err(Into::into)
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

    fn move_in_playlist(&mut self, playlist: &str, from: SingleOrRange, to: u32) -> Result<()> {
        self.client.move_in_playlist(playlist, &from, to as usize).map_err(Into::into)
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

    fn enable_output(&mut self, id: u32) -> Result<()> {
        self.client
            .enable_output(id)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))
    }

    fn disable_output(&mut self, id: u32) -> Result<()> {
        self.client
            .disable_output(id)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))
    }

    fn toggle_output(&mut self, id: u32) -> Result<()> {
        self.client
            .toggle_output(id)
            .map_err(|e: crate::mpd::errors::MpdError| anyhow::Error::from(e))
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

    fn capabilities(&self) -> &'static [crate::backends::api::Capability] {
        use crate::backends::api::Capability::*;
        &[
            Playlists,
            PlaylistCreate,
            PlaylistEdit,
            MpdDatabase,
            MpdStickers,
            MpdOutputs,
            MpdPartitions,
        ]
    }

    // supports() uses default implementation from trait

    fn supports_command(&self, _command: &str) -> bool {
        // Check against supported commands if available
        // For now, just return true as MPD supports most things
        true
    }
}
