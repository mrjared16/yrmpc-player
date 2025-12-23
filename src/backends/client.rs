use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::{
    config::{PlayerBackend, YouTubeConfig},
    domain::QueuePosition,
    app_state::AppState,
    mpd::{
        MpdClient,
        commands::{
            Decoder,
            IdleEvent,
            LsInfoEntry,
            OnOffOneshot,
            Output,
            Playlist,
            SaveMode,
            SeekPosition,
            Song,
            Tag,
            stickers::Sticker,
            ValueChange,
            list_mounts::Mount,
        },
        mpd_client::{Filter, Command, SingleOrRange},
        proto_client::ProtoClient,
    },
    backends::{MusicBackend, MpdBackend, youtube, BackendCapability},
    backends::interaction::{BackendActions, PartitionedOutput},
    backends::controllers::{
        PlaybackController, QueueController, StatusProvider, VolumeController,
        LibraryBrowser, SavedPlaylistController, StickerController,
        OutputController, DatabaseController,
    },
};
use std::sync::{Arc, RwLock};

/// Unified backend dispatcher that routes commands to the active backend.
///
/// # Overview
///
/// `BackendDispatcher` is the **main entry point** for all TUI-to-backend communication.
/// It abstracts away the differences between MPD and YouTube backends, providing a
/// unified interface for playback control, queue management, and library browsing.
///
/// # For New Developers
///
/// Think of this as a "smart switch" that knows which backend (MPD or YouTube) is
/// currently active and routes all commands appropriately.
///
/// ## Common Usage Patterns
///
/// ```ignore
/// // From TUI components, use via Ctx:
/// ctx.command(|dispatcher| {
///     dispatcher.play()?;
///     Ok(())
/// });
///
/// // Or use BackendActions trait for high-level operations:
/// BackendDispatcher::resolve_and_enqueue(ctx, items, position, autoplay, None, None);
/// ```
///
/// # Architecture
///
/// ```text
/// TUI Component
///     │
///     ▼
/// BackendDispatcher (this type)
///     │
///     ├──► MPD Backend (connects to external MPD server)
///     │
///     └──► YouTube Backend (connects to internal daemon via IPC)
/// ```
///
/// # Historical Note
///
/// This was previously named `BackendDispatcher`. The new name better reflects
/// its role as a dispatcher rather than a controller with business logic.
#[derive(Debug)]
pub enum BackendDispatcher<'name> {
    Mpd(MpdBackend<'name>),
    YouTube(youtube::YouTubeProxy),
}

impl<'name> BackendDispatcher<'name> {
    /// Create a new dispatcher using MPD backend
    pub fn new_mpd(client: crate::mpd::client::Client<'name>) -> Self {
        BackendDispatcher::Mpd(MpdBackend::new(client))
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
        Ok(BackendDispatcher::new_mpd(mpd_client))
    }

    /// Initialize dispatcher with backend selection from config
    pub fn init_with_backend(
        backend: PlayerBackend,
        addr: crate::config::MpdAddress,
        password: Option<crate::config::address::MpdPassword>,
        _mpv_socket: Option<String>,  // Deprecated - kept for API compat
        _youtube_config: YouTubeConfig,  // Deprecated - kept for API compat
        name: &'name str,
        partition: Option<String>,
        autocreate_partition: bool,
        _app_state: Arc<RwLock<AppState>>,  // Deprecated - kept for API compat
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
                Ok(BackendDispatcher::new_mpd(mpd_client))
            }
            PlayerBackend::YouTube => {
                let socket_path = std::path::Path::new("/tmp/yrmpc-yt.sock");
                let client = youtube::YouTubeProxy::connect(socket_path)?;
                Ok(BackendDispatcher::YouTube(client))
            }
        }
    }

    /// Get mutable reference to backend as trait object.
    /// 
    /// # Internal Use Only
    /// 
    /// This is an escape hatch for code that needs direct backend access.
    /// Prefer using the typed controller methods instead:
    /// - `playback()` for play/pause/stop
    /// - `queue()` for queue operations  
    /// - `status()` for status queries (returns rich `domain::Status`)
    /// - `volume_control()` for volume
    /// - `library()` for search/browse
    /// - `youtube()` for YouTube-specific features (browse details)
    /// 
    /// This method is `pub(crate)` because:
    /// 1. MPD-specific controllers (stickers, outputs, database) need it
    /// 2. StatusProvider needs it for rich `domain::Status`
    pub(crate) fn backend_mut(&mut self) -> &mut dyn MusicBackend {
        match self {
            BackendDispatcher::Mpd(b) => b as &mut dyn MusicBackend,
            BackendDispatcher::YouTube(b) => b as &mut dyn MusicBackend,
        }
    }

    // Helper to get immutable reference to backend as trait object
    fn backend(&self) -> &dyn MusicBackend {
        match self {
            BackendDispatcher::Mpd(b) => b as &dyn MusicBackend,
            BackendDispatcher::YouTube(b) => b as &dyn MusicBackend,
        }
    }

    // =========================================================================
    // CONTROLLER API - New organized interface
    // =========================================================================
    // 
    // These methods provide a clean, organized API grouped by functionality.
    // Use these instead of the flat method list below.
    //
    // Example:
    //   dispatcher.playback().play()?;
    //   dispatcher.queue().add(&song, None)?;
    //   if let Some(playlists) = dispatcher.saved_playlists() {
    //       playlists.save("favorites")?;
    //   }
    // =========================================================================

    /// Get playback controller for play/pause/stop/seek operations
    pub fn playback(&mut self) -> PlaybackController<'_> {
        match self {
            BackendDispatcher::Mpd(b) => PlaybackController { backend: b as &mut dyn api::Playback },
            BackendDispatcher::YouTube(b) => PlaybackController { backend: b as &mut dyn api::Playback },
        }
    }

    /// Get queue controller for queue management
    pub fn queue(&mut self) -> QueueController<'_> {
        match self {
            BackendDispatcher::Mpd(b) => QueueController { backend: b as &mut dyn api::Queue },
            BackendDispatcher::YouTube(b) => QueueController { backend: b as &mut dyn api::Queue },
        }
    }

    /// Get status provider for current state queries
    pub fn status(&mut self) -> StatusProvider<'_> {
        StatusProvider { backend: self.backend_mut() }
    }

    /// Get volume controller
    pub fn volume_control(&mut self) -> VolumeController<'_> {
        match self {
            BackendDispatcher::Mpd(b) => VolumeController { backend: b as &mut dyn api::Volume },
            BackendDispatcher::YouTube(b) => VolumeController { backend: b as &mut dyn api::Volume },
        }
    }

    /// Get library browser for search and browse operations
    pub fn library(&mut self) -> LibraryBrowser<'_> {
        match self {
            BackendDispatcher::Mpd(b) => LibraryBrowser { backend: b as &mut dyn api::Discovery },
            BackendDispatcher::YouTube(b) => LibraryBrowser { backend: b as &mut dyn api::Discovery },
        }
    }

    /// Get saved playlists controller (MPD only)
    /// 
    /// Returns `None` if the backend doesn't support saved playlists.
    pub fn saved_playlists(&mut self) -> Option<SavedPlaylistController<'_>> {
        if self.backend().supports(BackendCapability::SavedPlaylists) {
            Some(SavedPlaylistController { backend: self.backend_mut() })
        } else {
            None
        }
    }

    /// Get sticker controller for metadata operations (MPD only)
    /// 
    /// Returns `None` if the backend doesn't support stickers.
    pub fn stickers(&mut self) -> Option<StickerController<'_>> {
        if self.backend().supports(BackendCapability::Stickers) {
            Some(StickerController { backend: self.backend_mut() })
        } else {
            None
        }
    }

    /// Get output controller for audio output management (MPD only)
    /// 
    /// Returns `None` if the backend doesn't support output control.
    pub fn outputs_control(&mut self) -> Option<OutputController<'_>> {
        if self.backend().supports(BackendCapability::OutputControl) {
            Some(OutputController { backend: self.backend_mut() })
        } else {
            None
        }
    }

    /// Get database controller for update/rescan operations (MPD only)
    /// 
    /// Returns `None` if the backend doesn't support database management.
    pub fn database(&mut self) -> Option<DatabaseController<'_>> {
        if self.backend().supports(BackendCapability::DatabaseManagement) {
            Some(DatabaseController { backend: self.backend_mut() })
        } else {
            None
        }
    }

    /// Check if this backend supports a capability
    pub fn supports(&self, capability: BackendCapability) -> bool {
        self.backend().supports(capability)
    }

    /// Get the backend name (e.g., "MPD", "YouTube")
    pub fn name(&self) -> &'static str {
        self.backend().backend_name()
    }

    /// Get direct access to MPD backend (if using MPD)
    /// 
    /// Use this for advanced MPD-specific operations not covered by controllers.
    pub fn mpd(&mut self) -> Option<&mut MpdBackend<'name>> {
        match self {
            BackendDispatcher::Mpd(b) => Some(b),
            _ => None,
        }
    }

    /// Get direct access to YouTube backend (if using YouTube)
    /// 
    /// Use this for YouTube-specific operations not covered by controllers.
    pub fn youtube(&mut self) -> Option<&mut youtube::YouTubeProxy> {
        match self {
            BackendDispatcher::YouTube(b) => Some(b),
            _ => None,
        }
    }

    // =========================================================================
    // LEGACY API - Deprecated, use controllers above instead
    // =========================================================================
    // 
    // These methods are kept for backward compatibility.
    // They will be removed in a future version.
    //
    // Migration guide:
    //   OLD: dispatcher.play()?
    //   NEW: dispatcher.playback().play()?
    //
    //   OLD: dispatcher.add(uri, pos)?
    //   NEW: dispatcher.queue().add(uri, pos)?
    //
    //   OLD: dispatcher.get_status()?
    //   NEW: dispatcher.status().get()?
    // =========================================================================

    // ----- Playback (use playback() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().play() instead")]
    pub fn play(&mut self) -> Result<()> {
        self.backend_mut().play()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().pause(state) instead")]
    pub fn pause_state(&mut self, state: bool) -> Result<()> {
        self.backend_mut().pause(state)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().pause(true) instead")]
    pub fn pause(&mut self) -> Result<()> {
        self.pause_state(true)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().stop() instead")]
    pub fn stop(&mut self) -> Result<()> {
        self.backend_mut().stop()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().next() instead")]
    pub fn next(&mut self) -> Result<()> {
        self.backend_mut().next()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().previous() instead")]
    pub fn previous(&mut self) -> Result<()> {
        self.backend_mut().previous()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().seek(position) instead")]
    pub fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        self.backend_mut().seek_current(position)
    }

    // ----- Status (use status() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.status().get() instead")]
    pub fn get_status(&mut self) -> Result<crate::domain::Status> {
        self.backend_mut().get_status()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.status().queue() instead")]
    pub fn playlist_info(&mut self) -> Result<Vec<crate::domain::Song>> {
        self.backend_mut().playlist_info()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.status().current_song() instead")]
    pub fn current_song(&mut self) -> Result<Option<crate::domain::Song>> {
        self.backend_mut().current_song()
    }

    // ----- Queue (use queue() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().add(uri, position) instead")]
    pub fn add(&mut self, uri: &str, position: Option<crate::domain::QueuePosition>) -> Result<()> {
        self.backend_mut().add(uri, position)
    }

    /// Add song with full metadata (uses QueueOperations.enqueue)
    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().add_song(song, position) instead")]
    pub fn add_song(&mut self, song: &crate::domain::Song, position: Option<crate::domain::QueuePosition>) -> Result<()> {
        use crate::backends::QueueOperations;
        self.backend_mut().enqueue(song, position)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().delete(id) instead")]
    pub fn delete_id(&mut self, id: u32) -> Result<()> {
        self.backend_mut().delete_id(id)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().clear() instead")]
    pub fn clear(&mut self) -> Result<()> {
        self.backend_mut().clear()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().move_item(from, to) instead")]
    pub fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.backend_mut().move_id(from, to)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().play_id(id) instead")]
    pub fn play_id(&mut self, id: u32) -> Result<()> {
        self.backend_mut().play_id(id)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.queue().play_pos(pos) instead")]
    pub fn play_pos(&mut self, pos: usize) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.play_pos(pos).map_err(Into::into),
            BackendDispatcher::YouTube(b) => b.play_pos(pos),
        }
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().unpause() instead")]
    pub fn unpause(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.unpause().map_err(Into::into),
            BackendDispatcher::YouTube(_) => self.pause_state(false),
        }
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.status().current_song() instead")]
    pub fn get_current_song(&mut self) -> Result<Option<crate::domain::Song>> {
        self.current_song()
    }

    pub fn find_one(&mut self, filter: &[Filter]) -> Result<Option<crate::domain::Song>> {
        match self {
            BackendDispatcher::Mpd(b) => Ok(b.client.find_one(filter)?.map(Into::into)),
            BackendDispatcher::YouTube(_) => Ok(None),
        }
    }

    pub fn disable_output(&mut self, id: u32) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.disable_output(id).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    pub fn delete_all_stickers(&mut self, uri: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.delete_all_stickers(uri).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    pub fn config(&self) -> Option<crate::mpd::commands::mpd_config::MpdConfig> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.config.clone(),
            BackendDispatcher::YouTube(_) => None,
        }
    }

    // ----- Library (use library() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().suggestions(query) instead")]
    pub fn get_search_suggestions(&mut self, query: String) -> Result<Vec<String>> {
        self.backend_mut().get_search_suggestions(query)
    }

    // ----- Volume (use volume_control() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.volume_control().set(change) instead")]
    pub fn volume(&mut self, change: ValueChange) -> Result<()> {
        self.set_volume(change)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.volume_control().set(volume) instead")]
    pub fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        self.backend_mut().set_volume(volume)
    }

    // ----- Playback options (use playback() instead) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().repeat(repeat) instead")]
    pub fn repeat(&mut self, repeat: bool) -> Result<()> {
        self.backend_mut().repeat(repeat)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().random(random) instead")]
    pub fn random(&mut self, random: bool) -> Result<()> {
        self.backend_mut().random(random)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().single(single) instead")]
    pub fn single(&mut self, single: OnOffOneshot) -> Result<()> {
        self.backend_mut().single(single)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().consume(consume) instead")]
    pub fn consume(&mut self, consume: OnOffOneshot) -> Result<()> {
        self.backend_mut().consume(consume)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.playback().crossfade(seconds) instead")]
    pub fn crossfade(&mut self, seconds: u32) -> Result<()> {
        self.backend_mut().crossfade(seconds)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().browse(path) instead")]
    pub fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend_mut().lsinfo(path)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().by_category(category) instead")]
    pub fn get_library(&mut self, category: crate::player::LibraryCategory) -> Result<Vec<LsInfoEntry>> {
        self.backend_mut().get_library(category)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().list_all(path) instead")]
    pub fn list_all(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        self.backend_mut().list_all(path)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().search(filter) instead")]
    pub fn search(&mut self, filter: &[Filter]) -> Result<Vec<crate::domain::Song>> {
        self.backend_mut().search(filter)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.library().find(filter, window) instead")]
    pub fn find(
        &mut self,
        filter: &[Filter],
        window: Option<(u32, u32)>,
    ) -> Result<Vec<crate::domain::Song>> {
        self.backend_mut().find(filter, window)
    }

    // ----- Saved Playlists (use saved_playlists() instead - MPD only) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.list() instead")]
    pub fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        self.backend_mut().list_playlists()
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.get_songs(name) instead")]
    pub fn playlist_info_name(&mut self, name: &str) -> Result<Vec<crate::domain::Song>> {
        self.backend_mut().playlist_info_name(name)
    }

    pub fn list_tag(
        &mut self,
        tag: Tag,
        filter: Option<&[crate::mpd::mpd_client::Filter]>,
    ) -> Result<Vec<String>> {
        self.backend_mut().list_tag(tag, filter)
    }

    pub fn count(
        &mut self,
        filter: &[crate::mpd::mpd_client::Filter],
    ) -> Result<(usize, std::time::Duration)> {
        self.backend_mut().count(filter)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.load(name, position) instead")]
    pub fn load_playlist(&mut self, name: &str, position: Option<crate::domain::QueuePosition>) -> Result<()> {
        self.backend_mut().load_playlist(name, position)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.save(name, mode) instead")]
    pub fn save_queue_as_playlist(&mut self, name: &str, mode: Option<SaveMode>) -> Result<()> {
        self.backend_mut().save_queue_as_playlist(name, mode)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.delete(name) instead")]
    pub fn delete_playlist(&mut self, name: &str) -> Result<()> {
        self.backend_mut().delete_playlist(name)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.rename(old, new) instead")]
    pub fn rename_playlist(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        self.backend_mut().rename_playlist(old_name, new_name)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.add_song(playlist, uri) instead")]
    pub fn add_to_playlist(&mut self, playlist: &str, uri: &str) -> Result<()> {
        self.backend_mut().add_to_playlist(playlist, uri)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.delete_song(playlist, position) instead")]
    pub fn delete_from_playlist(&mut self, playlist: &str, position: u32) -> Result<()> {
        self.backend_mut().delete_from_playlist(playlist, position)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.saved_playlists()?.move_song(playlist, from, to) instead")]
    pub fn move_in_playlist(&mut self, playlist: &str, from: &SingleOrRange, to: usize) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.move_in_playlist(playlist, from, to).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    // ----- Stickers (use stickers() instead - MPD only) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.stickers()?.list(uri) instead")]
    pub fn list_stickers(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.backend_mut().list_stickers(uri)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.stickers()?.set(uri, key, value) instead")]
    pub fn set_sticker(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.backend_mut().set_sticker(uri, key, value)
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.stickers()?.delete(uri, key) instead")]
    pub fn delete_sticker(&mut self, uri: &str, key: &str) -> Result<()> {
        self.backend_mut().delete_sticker(uri, key)
    }

    // ----- Database (use database() instead - MPD only) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.database()?.update(path) instead")]
    pub fn update(&mut self, path: Option<&str>) -> Result<crate::mpd::commands::Update> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.update(path).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(crate::mpd::commands::Update { job_id: 0 }),
        }
    }

    #[deprecated(since = "0.12.0", note = "Use dispatcher.database()?.rescan(path) instead")]
    pub fn rescan(&mut self, path: Option<&str>) -> Result<crate::mpd::commands::Update> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.rescan(path).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(crate::mpd::commands::Update { job_id: 0 }),
        }
    }

    pub fn version(&self) -> crate::mpd::version::Version {
        self.backend().version()
    }

    // ----- Outputs (use outputs_control() instead - MPD only) -----

    #[deprecated(since = "0.12.0", note = "Use dispatcher.outputs_control()?.list() instead")]
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
            BackendDispatcher::Mpd(b) => b.client.supported_commands.clone(),
            BackendDispatcher::YouTube(_) => HashSet::new(),
        }
    }

    /// Get access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd(&self) -> Option<&crate::mpd::client::Client<'name>> {
        match self {
            BackendDispatcher::Mpd(b) => Some(&b.client),
            _ => None,
        }
    }

    /// Get mutable access to the underlying MPD client (if using MPD backend)
    pub fn as_mpd_mut(&mut self) -> Option<&mut crate::mpd::client::Client<'name>> {
        match self {
            BackendDispatcher::Mpd(b) => Some(&mut b.client),
            BackendDispatcher::YouTube(_) => None,
        }
    }

    /// Get mutable access to stream (MPD only)
    pub fn stream(&mut self) -> Option<&mut crate::mpd::client::TcpOrUnixStream> {
        match self {
            BackendDispatcher::Mpd(b) => Some(&mut b.client.stream),
            BackendDispatcher::YouTube(_) => None,
        }
    }

    /// Read from stream (MPD only)
    pub fn read(&mut self) -> Option<&mut std::io::BufReader<crate::mpd::client::TcpOrUnixStream>> {
        match self {
            BackendDispatcher::Mpd(b) => Some(&mut b.client.rx),
            BackendDispatcher::YouTube(_) => None,
        }
    }

    /// Set read timeout (MPD only)
    pub fn try_clone_stream(&self) -> Result<Box<dyn ClientStream>> {
        match self {
            BackendDispatcher::Mpd(b) => {
                let stream = b.client.stream.try_clone()?;
                Ok(Box::new(stream))
            }
            BackendDispatcher::YouTube(b) => {
                let stream = b.try_clone_stream()?;
                // Wrap in YouTubeStream so it doesn't write noidle
                Ok(Box::new(YouTubeStream(stream)))
            }
        }
    }

    pub fn enter_idle(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.enter_idle().map_err(Into::into),
            BackendDispatcher::YouTube(b) => b.enter_idle(),
        }
    }

    pub fn idle(&mut self, mask: Option<IdleEvent>) -> Result<Vec<IdleEvent>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.idle(mask).map_err(Into::into),
            BackendDispatcher::YouTube(b) => {
                b.enter_idle()?;
                b.read_response()
            }
        }
    }

    pub fn read_response(&mut self) -> Result<Vec<IdleEvent>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.read_response().map_err(Into::into),
            BackendDispatcher::YouTube(b) => b.read_response(),
        }
    }

    pub fn reconnect(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => {
                b.client.reconnect()?;
                Ok(())
            }
            BackendDispatcher::YouTube(b) => b.reconnect(),
        }
    }

    pub fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.set_read_timeout(duration).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()), // MPV doesn't have timeout settings
        }
    }

    pub fn set_write_timeout(&mut self, duration: Option<std::time::Duration>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.set_write_timeout(duration).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()), // MPV doesn't have timeout settings
        }
    }

    /// Get available commands (MPD only)
    pub fn commands(&mut self) -> Result<crate::mpd::commands::list::MpdList> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.commands().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(crate::mpd::commands::list::MpdList::default()),
        }
    }

    /// Get unavailable commands (MPD only)
    pub fn not_commands(&mut self) -> Result<crate::mpd::commands::list::MpdList> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.not_commands().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(crate::mpd::commands::list::MpdList::default()),
        }
    }

    pub fn list_partitioned_outputs(
        &mut self,
        current_partition: &str,
    ) -> Result<Vec<PartitionedOutput>> {
        match self {
            BackendDispatcher::Mpd(b) => {
                b.client.list_partitioned_outputs(current_partition).map_err(Into::into)
            }
            BackendDispatcher::YouTube(_) => Ok(Vec::new()),
        }
    }

    pub fn move_output(&mut self, output_name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.move_output(output_name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    pub fn enable_output(&mut self, id: u32) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.enable_output(id).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    pub fn toggle_output(&mut self, id: u32) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.toggle_output(id).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    pub fn add_to_playlist_multiple(
        &mut self,
        playlist: &str,
        uris: &[String],
        _target_position: Option<usize>,
    ) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => {
                b.client.add_to_playlist_multiple(playlist, uris.to_vec()).map_err(Into::into)
            }
            BackendDispatcher::YouTube(_) => Ok(()), // Not supported yet
        }
    }

    pub fn shuffle(&mut self, range: Option<SingleOrRange>) -> Result<()> {
        self.backend_mut().shuffle(range)
    }

    /// Get sticker value (MPD only)
    pub fn sticker(&mut self, uri: &str, key: &str) -> Result<Option<Sticker>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.sticker(uri, key).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(None),
        }
    }

    /// Get status (alias for status method)


    pub fn pause_toggle(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.pause_toggle().map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                // For YouTube: check current state and toggle
                let status = self.get_status()?;
                match status.state {
                    crate::domain::PlaybackState::Play => self.pause_state(true),
                    crate::domain::PlaybackState::Pause => self.pause_state(false),
                    crate::domain::PlaybackState::Stop => self.play(),
                }
            }
        }
    }

    pub fn move_in_queue(
        &mut self,
        from: crate::mpd::mpd_client::SingleOrRange,
        to: QueuePosition,
    ) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.move_in_queue(from, to.into()).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("move_in_queue not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    pub fn delete_from_queue(
        &mut self,
        range: crate::mpd::mpd_client::SingleOrRange,
    ) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.delete_from_queue(range).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("delete_from_queue not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    /// List songs in a playlist (MPD only)
    pub fn list_playlist_info(
        &mut self,
        playlist: &str,
        range: Option<SingleOrRange>,
    ) -> Result<Vec<crate::domain::Song>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client
                .list_playlist_info(playlist, range)
                .map(|songs| songs.into_iter().map(Into::into).collect())
                .map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                // MPV doesn't support playlists, return empty
                Ok(Vec::new())
            }
        }
    }

    /// Find stickers in the database (MPD only)
    pub fn find_stickers(
        &mut self,
        uri: &str,
        name: &str,
        filter: Option<crate::mpd::mpd_client::StickerFilter>,
    ) -> Result<crate::mpd::commands::stickers::StickersWithFile> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.find_stickers(uri, name, filter).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("find_stickers not supported in MPV/YouTube backend");
                Ok(crate::mpd::commands::stickers::StickersWithFile(Vec::new()))
            }
        }
    }

    /// Switch to a partition (MPD only)
    pub fn switch_to_partition(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.switch_to_partition(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("Partitions not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    /// Create a new partition (MPD only)
    pub fn new_partition(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.new_partition(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("Partitions not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    /// List partitions (MPD only)
    pub fn list_partitions(&mut self) -> Result<Vec<String>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.list_partitions().map(|l| l.0).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("Partitions not supported in MPV/YouTube backend");
                Ok(Vec::new())
            }
        }
    }

    /// Delete a partition (MPD only)
    pub fn delete_partition(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.delete_partition(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                log::debug!("Partitions not supported in MPV/YouTube backend");
                Ok(())
            }
        }
    }

    /// Send new partition command (MPD only)
    pub fn send_new_partition(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_new_partition(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Send switch to partition command (MPD only)
    pub fn send_switch_to_partition(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_switch_to_partition(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Send start command list (MPD only - for batching)
    pub fn send_start_cmd_list(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_start_cmd_list().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()), // MPV doesn't need command batching
        }
    }

    /// Send execute command list (MPD only - for batching)
    pub fn send_execute_cmd_list(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_execute_cmd_list().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()), // MPV doesn't need command batching
        }
    }

    /// Read OK response (MPD only - for batching)
    pub fn read_ok(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.read_ok().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()), // MPV doesn't need OK responses
        }
    }

    /// Send add command (MPD only - for batching)
    pub fn send_add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_add(uri, position.map(Into::into)).map_err(Into::into),
            BackendDispatcher::YouTube(_) => {
                // For MPV, just add directly
                self.add(uri, position)
            }
        }
    }

    /// Send playlist add command (MPD only - for batching)
    pub fn send_playlist_add(&mut self, playlist: &str, uri: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => {
                b.client.add_to_playlist(playlist, uri, None)?;
                Ok(())
            }
            BackendDispatcher::YouTube(_) => Ok(()), // Playlists not supported
        }
    }

    /// Read songs response (MPD only - for batching)
    pub fn read_songs_response(&mut self) -> Result<Vec<Song>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.read_response().map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(Vec::new()),
        }
    }

    /// Send lsinfo command (MPD only)
    pub fn send_lsinfo(&mut self, uri: Option<&str>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_lsinfo(uri).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Send delete from playlist command (MPD only)
    pub fn send_delete_from_playlist(
        &mut self,
        playlist: &str,
        range: &SingleOrRange,
    ) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_delete_from_playlist(playlist, range).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Find album art (MPD only)
    pub fn find_album_art(&mut self, uri: &str) -> Result<Option<Vec<u8>>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.find_album_art(uri).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(None),
        }
    }

    /// Send list_all command (MPD only)
    pub fn send_list_all(&mut self, path: Option<&str>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_list_all(path).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Mount storage (MPD only)
    pub fn mount(&mut self, name: &str, path: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.mount(name, path).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Unmount storage (MPD only)
    pub fn unmount(&mut self, name: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.unmount(name).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// List mounts (MPD only)
    pub fn list_mounts(&mut self) -> Result<Vec<Mount>> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.list_mounts().map(|m| m.0).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(Vec::new()),
        }
    }

    /// Send message to channel (MPD only)
    pub fn send_message(&mut self, channel: &str, message: &str) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_message(channel, message).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Add random songs by tag (MPD only)
    pub fn add_random_tag(&mut self, count: usize, tag: Tag) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.add_random_tag(count, tag).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Add random songs (MPD only)
    pub fn add_random_songs(&mut self, count: usize, filter: Option<&[Filter]>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.add_random_songs(count, filter).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }

    /// Send find and add command (MPD only)
    pub fn send_find_add(&mut self, filter: &[Filter], position: Option<QueuePosition>) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => b.client.send_find_add(filter, position.map(Into::into)).map_err(Into::into),
            BackendDispatcher::YouTube(_) => Ok(()),
        }
    }
}

//=============================================================================
// API TRAIT IMPLEMENTATIONS
//=============================================================================
//
// These provide the new backend-agnostic interface defined in api/.
// They delegate to the underlying backend's API implementation.
//
// Note: The match pattern is intentionally explicit rather than using a macro.
// With only 2 backends, explicit code is clearer than macro magic.
// If we add more backends, consider extracting to a dispatch macro.

use crate::backends::api::{self, Item, SearchQuery, SearchResults, BrowseResult, Capability, InsertAt, AfterAdd};

impl api::Playback for BackendDispatcher<'_> {
    fn play(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::play(b),
            BackendDispatcher::YouTube(b) => api::Playback::play(b),
        }
    }

    fn pause(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::pause(b),
            BackendDispatcher::YouTube(b) => api::Playback::pause(b),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::stop(b),
            BackendDispatcher::YouTube(b) => api::Playback::stop(b),
        }
    }

    fn next(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::next(b),
            BackendDispatcher::YouTube(b) => api::Playback::next(b),
        }
    }

    fn previous(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::previous(b),
            BackendDispatcher::YouTube(b) => api::Playback::previous(b),
        }
    }

    fn seek(&mut self, position: std::time::Duration) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::seek(b, position),
            BackendDispatcher::YouTube(b) => api::Playback::seek(b, position),
        }
    }

    fn seek_relative(&mut self, delta_secs: i64) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::seek_relative(b, delta_secs),
            BackendDispatcher::YouTube(b) => api::Playback::seek_relative(b, delta_secs),
        }
    }

    fn status(&mut self) -> Result<api::Status> {
        match self {
            BackendDispatcher::Mpd(b) => api::Playback::status(b),
            BackendDispatcher::YouTube(b) => api::Playback::status(b),
        }
    }
}

impl api::Queue for BackendDispatcher<'_> {
    fn add(&mut self, items: &[Item], at: InsertAt, after: AfterAdd) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::add(b, items, at, after),
            BackendDispatcher::YouTube(b) => api::Queue::add(b, items, at, after),
        }
    }

    fn remove(&mut self, queue_ids: &[u32]) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::remove(b, queue_ids),
            BackendDispatcher::YouTube(b) => api::Queue::remove(b, queue_ids),
        }
    }

    fn list(&mut self) -> Result<Vec<Item>> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::list(b),
            BackendDispatcher::YouTube(b) => api::Queue::list(b),
        }
    }

    fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::move_items(b, queue_ids, to_position),
            BackendDispatcher::YouTube(b) => api::Queue::move_items(b, queue_ids, to_position),
        }
    }

    fn clear(&mut self) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::clear(b),
            BackendDispatcher::YouTube(b) => api::Queue::clear(b),
        }
    }

    fn play_id(&mut self, queue_id: u32) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::play_id(b, queue_id),
            BackendDispatcher::YouTube(b) => api::Queue::play_id(b, queue_id),
        }
    }

    fn set_repeat(&mut self, mode: api::Repeat) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::set_repeat(b, mode),
            BackendDispatcher::YouTube(b) => api::Queue::set_repeat(b, mode),
        }
    }

    fn set_shuffle(&mut self, enabled: bool) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Queue::set_shuffle(b, enabled),
            BackendDispatcher::YouTube(b) => api::Queue::set_shuffle(b, enabled),
        }
    }
}

impl api::Discovery for BackendDispatcher<'_> {
    fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        match self {
            BackendDispatcher::Mpd(b) => api::Discovery::search(b, query),
            BackendDispatcher::YouTube(b) => api::Discovery::search(b, query),
        }
    }

    fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        match self {
            BackendDispatcher::Mpd(b) => api::Discovery::browse(b, path),
            BackendDispatcher::YouTube(b) => api::Discovery::browse(b, path),
        }
    }

    fn suggestions(&mut self, partial: &str) -> Result<Vec<String>> {
        match self {
            BackendDispatcher::Mpd(b) => api::Discovery::suggestions(b, partial),
            BackendDispatcher::YouTube(b) => api::Discovery::suggestions(b, partial),
        }
    }

    fn resolve(&mut self, item: &Item) -> Result<Vec<Item>> {
        match self {
            BackendDispatcher::Mpd(b) => api::Discovery::resolve(b, item),
            BackendDispatcher::YouTube(b) => api::Discovery::resolve(b, item),
        }
    }

    fn details(&mut self, item: &Item) -> Result<crate::domain::ContentDetails> {
        match self {
            BackendDispatcher::Mpd(b) => api::Discovery::details(b, item),
            BackendDispatcher::YouTube(b) => api::Discovery::details(b, item),
        }
    }
}

impl api::Volume for BackendDispatcher<'_> {
    fn get(&mut self) -> Result<u8> {
        match self {
            BackendDispatcher::Mpd(b) => api::Volume::get(b),
            BackendDispatcher::YouTube(b) => api::Volume::get(b),
        }
    }

    fn set(&mut self, volume: u8) -> Result<()> {
        match self {
            BackendDispatcher::Mpd(b) => api::Volume::set(b, volume),
            BackendDispatcher::YouTube(b) => api::Volume::set(b, volume),
        }
    }
}

impl api::Backend for BackendDispatcher<'_> {
    fn name(&self) -> &'static str {
        match self {
            BackendDispatcher::Mpd(b) => api::Backend::name(b),
            BackendDispatcher::YouTube(b) => api::Backend::name(b),
        }
    }

    fn capabilities(&self) -> &[Capability] {
        match self {
            BackendDispatcher::Mpd(b) => api::Backend::capabilities(b),
            BackendDispatcher::YouTube(b) => api::Backend::capabilities(b),
        }
    }
}

pub trait ClientStream: std::io::Write + Send {
    fn shutdown_both(&mut self) -> std::io::Result<()>;
    
    /// Write MPD "noidle" command. Returns Ok without writing for non-MPD backends.
    fn write_noidle(&mut self) -> std::io::Result<()> {
        // Default implementation writes noidle for MPD compatibility
        self.write_all(b"noidle\n")?;
        self.flush()
    }
}

impl ClientStream for crate::mpd::client::TcpOrUnixStream {
    fn shutdown_both(&mut self) -> std::io::Result<()> {
        self.shutdown_both()
    }
    // Uses default write_noidle (writes the command)
}

/// Wrapper for YouTube Unix stream that doesn't write noidle
pub struct YouTubeStream(pub std::os::unix::net::UnixStream);

impl std::io::Write for YouTubeStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl ClientStream for YouTubeStream {
    fn shutdown_both(&mut self) -> std::io::Result<()> {
        self.0.shutdown(std::net::Shutdown::Both)
    }
    
    /// YouTube doesn't use MPD idle protocol, so don't write noidle
    fn write_noidle(&mut self) -> std::io::Result<()> {
        Ok(()) // No-op for YouTube
    }
}

impl ClientStream for std::os::unix::net::UnixStream {
    fn shutdown_both(&mut self) -> std::io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}
