use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::{
    config::PlayerBackend,
    mpd::{
        MpdClient,
        commands::{
            Decoder,
            IdleEvent,
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
        },
        mpd_client::{MpdClient as MpdClientTrait, SingleOrRange},
    },
    player::{backend::MusicBackend, mpd_backend::MpdBackend, mpv_backend::MpvBackend},
    shared::mpd_client_ext::{MpdClientExt, PartitionedOutput},
};

/// Unified client that can use either MPD or MPV backend
#[derive(Debug)]
pub enum Client<'name> {
    Mpd(MpdBackend<'name>),
    Mpv(MpvBackend),
}

impl<'name> Client<'name> {
    /// Create a new client using MPD backend
    pub fn new_mpd(client: crate::mpd::client::Client<'name>) -> Self {
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
        let mpd_client = crate::mpd::client::Client::init(
            addr,
            password,
            name,
            partition,
            autocreate_partition,
        )?;
        Ok(Client::new_mpd(mpd_client))
    }

    /// Initialize client with backend selection from config
    pub fn init_with_backend(
        backend: PlayerBackend,
        addr: crate::config::MpdAddress,
        password: Option<crate::config::address::MpdPassword>,
        mpv_socket: Option<String>,
        name: &'name str,
        partition: Option<String>,
        autocreate_partition: bool,
    ) -> Result<Self> {
        match backend {
            PlayerBackend::Mpd => {
                let mpd_client = crate::mpd::client::Client::init(
                    addr,
                    password,
                    name,
                    partition,
                    autocreate_partition,
                )?;
                Ok(Client::new_mpd(mpd_client))
            }
            PlayerBackend::Mpv => {
                let socket = mpv_socket.unwrap_or_else(|| "/tmp/rmpc-mpv.sock".to_string());
                Client::new_mpv(&socket)
            }
        }
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

    pub fn search(&mut self, filter: &[crate::mpd::mpd_client::Filter]) -> Result<Vec<Song>> {
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

    pub fn save_queue_as_playlist(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()> {
        self.backend_mut().save_queue_as_playlist(name, mode)
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

    /// Get supported commands (MPD only)
    pub fn supported_commands(&self) -> HashSet<String> {
        match self {
            Client::Mpd(b) => b.client.supported_commands.clone(),
            Client::Mpv(_) => HashSet::new(),
        }
    }

    /// Get access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd(&self) -> Option<&crate::mpd::client::Client<'name>> {
        match self {
            Client::Mpd(b) => Some(&b.client),
            _ => None,
        }
    }

    /// Get mutable access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd_mut(&mut self) -> Option<&mut crate::mpd::client::Client<'name>> {
        match self {
            Client::Mpd(b) => Some(&mut b.client),
            Client::Mpv(_) => None,
        }
    }

    /// Get mutable access to stream (MPD only)
    pub fn stream(&mut self) -> Option<&mut crate::mpd::client::TcpOrUnixStream> {
        match self {
            Client::Mpd(b) => Some(&mut b.client.stream),
            Client::Mpv(_) => None,
        }
    }

    /// Read from stream (MPD only)
    pub fn read(&mut self) -> Option<&mut std::io::BufReader<crate::mpd::client::TcpOrUnixStream>> {
        match self {
            Client::Mpd(b) => Some(&mut b.client.rx),
            Client::Mpv(_) => None,
        }
    }

    /// Set read timeout (MPD only)
    pub fn try_clone_stream(&self) -> Result<Box<dyn ClientStream>> {
        match self {
            Client::Mpd(b) => {
                let stream = b.client.stream.try_clone()?;
                Ok(Box::new(stream))
            }
            Client::Mpv(b) => {
                let stream = b.try_clone_stream()?;
                Ok(Box::new(stream))
            }
        }
    }

    pub fn enter_idle(&mut self) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.enter_idle().map_err(Into::into),
            Client::Mpv(b) => b.enter_idle(),
        }
    }

    pub fn read_response(&mut self) -> Result<Vec<IdleEvent>> {
        match self {
            Client::Mpd(b) => b.client.read_idle_response().map_err(Into::into),
            Client::Mpv(b) => b.read_response(),
        }
    }

    pub fn reconnect(&mut self) -> Result<()> {
        match self {
            Client::Mpd(b) => {
                b.client.reconnect()?;
                Ok(())
            }
            Client::Mpv(b) => b.reconnect(),
        }
    }

    pub fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.set_read_timeout(duration).map_err(Into::into),
            Client::Mpv(_) => Ok(()), // MPV doesn't have timeout settings
        }
    }

    pub fn set_write_timeout(&mut self, duration: Option<std::time::Duration>) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.set_write_timeout(duration).map_err(Into::into),
            Client::Mpv(_) => Ok(()), // MPV doesn't have timeout settings
        }
    }

    /// Get available commands (MPD only)
    pub fn commands(&mut self) -> Result<crate::mpd::commands::list::MpdList> {
        match self {
            Client::Mpd(b) => b.client.commands().map_err(Into::into),
            Client::Mpv(_) => Ok(crate::mpd::commands::list::MpdList::default()),
        }
    }

    /// Get unavailable commands (MPD only)
    pub fn not_commands(&mut self) -> Result<crate::mpd::commands::list::MpdList> {
        match self {
            Client::Mpd(b) => b.client.not_commands().map_err(Into::into),
            Client::Mpv(_) => Ok(crate::mpd::commands::list::MpdList::default()),
        }
    }

    pub fn list_partitioned_outputs(
        &mut self,
        current_partition: &str,
    ) -> Result<Vec<PartitionedOutput>> {
        match self {
            Client::Mpd(b) => {
                b.client.list_partitioned_outputs(current_partition).map_err(Into::into)
            }
            Client::Mpv(_) => Ok(Vec::new()),
        }
    }

    pub fn move_output(&mut self, output_name: &str) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.move_output(output_name).map_err(Into::into),
            Client::Mpv(_) => Ok(()),
        }
    }

    pub fn enable_output(&mut self, id: u32) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.enable_output(id).map_err(Into::into),
            Client::Mpv(_) => Ok(()),
        }
    }

    pub fn toggle_output(&mut self, id: u32) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.toggle_output(id).map_err(Into::into),
            Client::Mpv(_) => Ok(()),
        }
    }

    pub fn add_to_playlist_multiple(
        &mut self,
        playlist: &str,
        uris: &[String],
        target_position: Option<usize>,
    ) -> Result<()> {
        match self {
            Client::Mpd(b) => {
                b.client.add_to_playlist_multiple(playlist, uris.to_vec()).map_err(Into::into)
            }
            Client::Mpv(_) => Ok(()), // Not supported yet
        }
    }

    pub fn shuffle(&mut self, range: Option<SingleOrRange>) -> Result<()> {
        self.backend_mut().shuffle(range)
    }

    /// Get sticker value (MPD only)
    pub fn sticker(&mut self, uri: &str, key: &str) -> Result<String> {
        match self {
            Client::Mpd(b) => b.client.sticker(uri, key).map_err(Into::into),
            Client::Mpv(_) => Err(anyhow::anyhow!("Stickers not supported in MPV backend")),
        }
    }

    /// Get status (alias for status method)
    pub fn get_status(&mut self) -> Result<Status> {
        self.status()
    }

    /// List songs in a playlist (MPD only)
    pub fn list_playlist_info(
        &mut self,
        playlist: &str,
        range: Option<SingleOrRange>,
    ) -> Result<Vec<Song>> {
        match self {
            Client::Mpd(b) => b.client.list_playlist_info(playlist, range).map_err(Into::into),
            Client::Mpv(_) => {
                log::debug!("list_playlist_info not supported in MPV backend");
                Ok(Vec::new())
            }
        }
    }

    /// Find stickers in the database (MPD only)
    pub fn find_stickers(
        &mut self,
        uri: &str,
        name: &str,
        filter: Option<crate::mpd::commands::sticker::StickerFilter>,
    ) -> Result<crate::mpd::commands::sticker::StickersWithFile> {
        match self {
            Client::Mpd(b) => b.client.find_stickers(uri, name, filter).map_err(Into::into),
            Client::Mpv(_) => {
                log::debug!("find_stickers not supported in MPV backend");
                Ok(crate::mpd::commands::sticker::StickersWithFile(Vec::new()))
            }
        }
    }

    /// Switch to a partition (MPD only)
    pub fn switch_to_partition(&mut self, name: &str) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.switch_to_partition(name).map_err(Into::into),
            Client::Mpv(_) => {
                log::debug!("Partitions not supported in MPV backend");
                Ok(())
            }
        }
    }

    /// Send start command list (MPD only - for batching)
    pub fn send_start_cmd_list(&mut self) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.send_start_cmd_list().map_err(Into::into),
            Client::Mpv(_) => Ok(()), // MPV doesn't need command batching
        }
    }

    /// Send execute command list (MPD only - for batching)
    pub fn send_execute_cmd_list(&mut self) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.send_execute_cmd_list().map_err(Into::into),
            Client::Mpv(_) => Ok(()), // MPV doesn't need command batching
        }
    }

    /// Read OK response (MPD only - for batching)
    pub fn read_ok(&mut self) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.read_ok().map_err(Into::into),
            Client::Mpv(_) => Ok(()), // MPV doesn't need OK responses
        }
    }

    /// Send add command (MPD only - for batching)
    pub fn send_add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.send_add(uri, position).map_err(Into::into),
            Client::Mpv(_) => {
                // For MPV, just add directly
                self.add(uri, position)
            }
        }
    }

    /// Send playlist add command (MPD only - for batching)
    pub fn send_playlist_add(&mut self, playlist: &str, uri: &str) -> Result<()> {
        match self {
            Client::Mpd(b) => b.client.send_playlist_add(playlist, uri).map_err(Into::into),
            Client::Mpv(_) => Ok(()), // Playlists not supported
        }
    }
}

pub trait ClientStream: std::io::Write + Send {
    fn shutdown_both(&mut self) -> std::io::Result<()>;
}

impl ClientStream for crate::mpd::client::TcpOrUnixStream {
    fn shutdown_both(&mut self) -> std::io::Result<()> {
        self.shutdown_both()
    }
}

impl ClientStream for std::os::unix::net::UnixStream {
    fn shutdown_both(&mut self) -> std::io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}
