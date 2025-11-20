use std::collections::HashMap;

use anyhow::Result;

use super::{backend::MusicBackend, mpd_backend::MpdBackend, mpv_backend::MpvBackend};
use crate::mpd::{client::Client as MpdClient, commands::*};

/// Unified client that can use either MPD or MPV backend
pub enum Client<'name> {
    Mpd(MpdBackend<'name>),
    Mpv(MpvBackend),
}

impl<'name> Client<'name> {
    /// Create a new client using MPD backend
    pub fn new_mpd(client: MpdClient<'name>) -> Self {
        Client::Mpd(MpdBackend::new(client))
    }

    /// Create a new client using MPV backend
    pub fn new_mpv(socket_path: &str) -> Result<Self> {
        Ok(Client::Mpv(MpvBackend::new(socket_path)?))
    }

    /// Initialize MPD client (convenience method)
    pub fn init(
        addr: crate::config::MpdAddress,
        password: Option<crate::config::address::MpdPassword>,
        name: &'name str,
        partition: Option<String>,
        autocreate_partition: bool,
    ) -> Result<Self> {
        let mpd_client = MpdClient::init(addr, password, name, partition, autocreate_partition)?;
        Ok(Client::new_mpd(mpd_client))
    }

    // Helper to get mutable reference to backend as trait object
    fn backend_mut(&mut self) -> &mut dyn MusicBackend {
        match self {
            Client::Mpd(b) => b as &mut dyn MusicBackend,
            Client::Mpv(b) => b as &mut dyn MusicBackend,
        }
    }

    // Helper to get immutable reference to backend as trait object
    fn backend(&self) -> &dyn MusicBackend {
        match self {
            Client::Mpd(b) => b as &dyn MusicBackend,
            Client::Mpv(b) => b as &dyn MusicBackend,
        }
    }

    // Delegate all methods to the backend trait

    pub fn play(&mut self) -> Result<()> {
        self.backend_mut().play()
    }

    pub fn pause(&mut self, state: bool) -> Result<()> {
        self.backend_mut().pause(state)
    }

    pub fn stop(&mut self) -> Result<()> {
        self.backend_mut().stop()
    }

    pub fn next(&mut self) -> Result<()> {
        self.backend_mut().next()
    }

    pub fn previous(&mut self) -> Result<()> {
        self.backend_mut().previous()
    }

    pub fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        self.backend_mut().seek_current(position)
    }

    pub fn status(&mut self) -> Result<Status> {
        self.backend_mut().get_status()
    }

    pub fn playlist_info(&mut self) -> Result<Vec<Song>> {
        self.backend_mut().playlist_info()
    }

    pub fn current_song(&mut self) -> Result<Option<Song>> {
        self.backend_mut().current_song()
    }

    pub fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        self.backend_mut().add(uri, position)
    }

    pub fn delete_id(&mut self, id: u32) -> Result<()> {
        self.backend_mut().delete_id(id)
    }

    pub fn clear(&mut self) -> Result<()> {
        self.backend_mut().clear()
    }

    pub fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.backend_mut().move_id(from, to)
    }

    pub fn play_id(&mut self, id: u32) -> Result<()> {
        self.backend_mut().play_id(id)
    }

    pub fn volume(&mut self) -> Result<u8> {
        self.backend_mut().volume()
    }

    pub fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        self.backend_mut().set_volume(volume)
    }

    pub fn repeat(&mut self, repeat: bool) -> Result<()> {
        self.backend_mut().repeat(repeat)
    }

    pub fn random(&mut self, random: bool) -> Result<()> {
        self.backend_mut().random(random)
    }

    pub fn single(&mut self, single: OnOffOneshot) -> Result<()> {
        self.backend_mut().single(single)
    }

    pub fn consume(&mut self, consume: OnOffOneshot) -> Result<()> {
        self.backend_mut().consume(consume)
    }

    pub fn crossfade(&mut self, seconds: u32) -> Result<()> {
        self.backend_mut().crossfade(seconds)
    }

    pub fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend_mut().lsinfo(path)
    }

    pub fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend_mut().list_all(path)
    }

    pub fn search(&mut self, filter: &[(Tag, String)]) -> Result<Vec<Song>> {
        self.backend_mut().search(filter)
    }

    pub fn find(
        &mut self,
        filter: &[(Tag, String)],
        window: Option<(u32, u32)>,
    ) -> Result<Vec<Song>> {
        self.backend_mut().find(filter, window)
    }

    pub fn list_tag(&mut self, tag: Tag, filter: Option<&[(Tag, String)]>) -> Result<Vec<String>> {
        self.backend_mut().list_tag(tag, filter)
    }

    pub fn count(&mut self, filter: &[(Tag, String)]) -> Result<(usize, std::time::Duration)> {
        self.backend_mut().count(filter)
    }

    pub fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        self.backend_mut().list_playlists()
    }

    pub fn playlist_info_name(&mut self, name: &str) -> Result<Vec<Song>> {
        self.backend_mut().playlist_info_name(name)
    }

    pub fn load_playlist(&mut self, name: &str, position: Option<QueuePosition>) -> Result<()> {
        self.backend_mut().load_playlist(name, position)
    }

    pub fn save_queue_as_playlist(&mut self, name: &str) -> Result<()> {
        self.backend_mut().save_queue_as_playlist(name)
    }

    pub fn delete_playlist(&mut self, name: &str) -> Result<()> {
        self.backend_mut().delete_playlist(name)
    }

    pub fn rename_playlist(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.backend_mut().rename_playlist(old_name, new_name)
    }

    pub fn add_to_playlist(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.backend_mut().add_to_playlist(playlist, uri)
    }

    pub fn delete_from_playlist(&mut self, playlist: &str, position: u32) -> Result<()> {
        self.backend_mut().delete_from_playlist(playlist, position)
    }

    pub fn move_in_playlist(&mut self, playlist: &str, from: u32, to: u32) -> Result<()> {
        self.backend_mut().move_in_playlist(playlist, from, to)
    }

    pub fn list_stickers(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.backend_mut().list_stickers(uri)
    }

    pub fn set_sticker(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.backend_mut().set_sticker(uri, key, value)
    }

    pub fn delete_sticker(&mut self, uri: &str, key: &str) -> Result<()> {
        self.backend_mut().delete_sticker(uri, key)
    }

    pub fn update(&mut self, path: Option<&str>) -> Result<u32> {
        self.backend_mut().update(path)
    }

    pub fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        self.backend_mut().rescan(path)
    }

    pub fn version(&self) -> crate::mpd::version::Version {
        self.backend().version()
    }

    pub fn outputs(&mut self) -> Result<Vec<Output>> {
        self.backend_mut().outputs()
    }

    pub fn decoders(&mut self) -> Result<Vec<Decoder>> {
        self.backend_mut().decoders()
    }

    pub fn partitions(&mut self) -> Result<Vec<String>> {
        self.backend_mut().partitions()
    }

    /// Get access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd(&self) -> Option<&MpdClient<'name>> {
        match self {
            Client::Mpd(b) => Some(&b.client),
            Client::Mpv(_) => None,
        }
    }

    /// Get mutable access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd_mut(&mut self) -> Option<&mut MpdClient<'name>> {
        match self {
            Client::Mpd(b) => Some(&mut b.client),
            Client::Mpv(_) => None,
        }
    }

    /// Set read timeout (MPD only)
    pub fn set_read_timeout(&mut self, timeout: Option<std::time::Duration>) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.set_read_timeout(timeout),
            Client::Mpv(_) => Ok(()), // No-op for MPV
        }
    }

    /// Set write timeout (MPD only)
    pub fn set_write_timeout(&mut self, timeout: Option<std::time::Duration>) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.set_write_timeout(timeout),
            Client::Mpv(_) => Ok(()), // No-op for MPV
        }
    }
}
