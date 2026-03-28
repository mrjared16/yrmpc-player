use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::AddAssign,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bon::bon;
use crossbeam::channel::{SendError, Sender, bounded};

use crate::{
    AppEvent, PlayerCommand, Query, QueryResult, WorkRequest,
    backends::{BackendActions, BackendDispatcher, QuerySync, api::Capability},
    config::{
        Config,
        album_art::ImageMethod,
        tabs::{PaneType, TabName},
    },
    core::{
        controllers::Controllers,
        queue_store::QueueStore,
        scheduler::{Scheduler, time_provider::DefaultTimeProvider},
    },
    domain::{PlaybackState as State, Song, Status},
    mpd::version::Version,
    shared::{
        events::ClientRequest,
        image_cache::ImageCache,
        lrc::{Lrc, LrcIndex, get_lrc_path},
        macros::{status_error, status_warn},
        ring_vec::RingVec,
    },
    ui::StatusMessage,
};

pub const FETCH_SONG_STICKERS: &str = "fetch_song_stickers";
pub const LIKE_STICKER: &str = "like";
pub const RATING_STICKER: &str = "rating";

#[derive(derive_more::Debug)]
pub struct Ctx {
    pub(crate) backend_version: Version,
    pub(crate) config: std::sync::Arc<Config>,
    pub(crate) status: Status,
    pub(crate) image_cache: ImageCache,
    /// Application state with in-memory queue management
    pub(crate) app_state: Arc<RwLock<crate::app_state::AppState>>,
    #[debug(skip)]
    pub(crate) controllers: Controllers,
    #[cfg(test)]
    pub(crate) stickers: HashMap<String, HashMap<String, String>>,
    #[cfg(not(test))]
    stickers: HashMap<String, HashMap<String, String>>,
    pub(crate) active_tab: TabName,
    pub(crate) supported_commands: HashSet<String>,
    /// Backend capabilities (cached at init from backend)
    pub(crate) capabilities: &'static [Capability],
    pub(crate) db_update_start: Option<Instant>,
    #[debug(skip)]
    pub(crate) app_event_sender: Sender<AppEvent>,
    #[debug(skip)]
    pub(crate) work_sender: Sender<WorkRequest>,
    #[debug(skip)]
    pub(crate) client_request_sender: Sender<ClientRequest>,
    pub(crate) needs_render: Cell<bool>,
    pub(crate) stickers_to_fetch: RefCell<HashSet<String>>,
    #[debug(skip)]
    pub(crate) lrc_index: LrcIndex,
    pub(crate) rendered_frames: u64,
    #[debug(skip)]
    pub(crate) scheduler: Scheduler<(Sender<AppEvent>, Sender<ClientRequest>), DefaultTimeProvider>,
    pub(crate) messages: RingVec<10, StatusMessage>,
    pub(crate) last_status_update: Instant,
    pub(crate) song_played: Option<Duration>,
    pub(crate) stickers_supported: StickersSupport,
    pub(crate) debug_ui_log: Option<std::path::PathBuf>,
    /// Whether the queue side panel is visible (toggle with 'Q')
    pub(crate) queue_panel_visible: bool,
    /// Previous tab before expanding to Queue (for back navigation with 'h')
    pub(crate) previous_tab: Option<TabName>,
}

#[bon]
impl Ctx {
    pub(crate) fn try_new(
        client: &mut BackendDispatcher<'_>,
        mut config: Config,
        app_event_sender: Sender<AppEvent>,
        work_sender: Sender<WorkRequest>,
        client_request_sender: Sender<ClientRequest>,
        mut scheduler: Scheduler<(Sender<AppEvent>, Sender<ClientRequest>), DefaultTimeProvider>,
        app_state: Arc<RwLock<crate::app_state::AppState>>,
    ) -> Result<Self> {
        let supported_commands: HashSet<String> = client.supported_commands();
        let stickers_supported = {
            // Use capability system for cleaner backend-agnostic check
            if client.supports(Capability::MpdStickers) {
                StickersSupport::Supported
            } else {
                // YouTube and other streaming backends don't support MPD stickers
                StickersSupport::UnsupportedAndChecked
            }
        };
        log::info!(supported_commands:? = supported_commands; "Supported commands by server");

        // Capture backend capabilities at init time (static, never changes)
        let capabilities = client.capabilities();
        log::info!(capabilities:? = capabilities; "Backend capabilities");

        let image_cache = ImageCache::new(app_event_sender.clone());

        let status = client.get_status()?;
        let queue = client.playlist_info()?;

        // Album art check - only warn for MPD backend (YouTube uses thumbnails from
        // API)
        if config.backend == crate::config::PlayerBackend::Mpd {
            if !supported_commands.contains("albumart")
                && !supported_commands.contains("readpicture")
            {
                config.album_art.method = ImageMethod::None;
                status_warn!("Album art is disabled because it is not supported by MPD server");
            }
        }

        log::info!(config:? = config; "Resolved config");

        let controllers = Controllers::new(
            queue.clone(),
            app_event_sender.clone(),
            client_request_sender.clone(),
        );

        let active_tab = config.tabs.names.first().context("Expected at least one tab")?.clone();
        scheduler.start();
        Ok(Self {
            backend_version: client.version(),
            lrc_index: LrcIndex::default(),
            config: std::sync::Arc::new(config),
            status,
            app_state,
            controllers,
            stickers: HashMap::new(),
            active_tab,
            supported_commands,
            capabilities,
            db_update_start: None,
            app_event_sender,
            work_sender,
            scheduler,
            image_cache,
            client_request_sender,
            needs_render: Cell::new(false),
            stickers_to_fetch: RefCell::new(HashSet::new()),
            rendered_frames: 0,
            messages: RingVec::default(),
            song_played: None,
            last_status_update: Instant::now(),
            stickers_supported,
            debug_ui_log: None,
            queue_panel_visible: false,
            previous_tab: None,
        })
    }

    pub(crate) fn set_debug_ui_log(&mut self, path: Option<std::path::PathBuf>) {
        self.debug_ui_log = path;
    }

    pub fn queue_store(&self) -> &QueueStore {
        &self.controllers.queue
    }

    // =========================================================================
    // BACKEND CAPABILITY CHECKS
    // =========================================================================
    // Use ctx.supports(BackendCapability::X) to check if features are available.

    /// Check if the current backend is MPD
    pub fn is_mpd(&self) -> bool {
        matches!(self.config.backend, crate::config::PlayerBackend::Mpd)
    }

    /// Check if the current backend is YouTube
    pub fn is_youtube(&self) -> bool {
        matches!(self.config.backend, crate::config::PlayerBackend::YouTube)
    }

    /// Check if this backend supports a specific capability.
    ///
    /// Uses static slice lookup - O(n) for n=6 capabilities, faster than
    /// HashSet for small n.
    ///
    /// # Example
    /// ```ignore
    /// if !ctx.supports(BackendCapability::Stickers) {
    ///     status_warn!("Stickers not supported by this backend");
    ///     return Ok(());
    /// }
    /// ```
    pub fn supports(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    // TODO: Error comes from crossebeam, try to remove later if it gets solved
    // upstream
    #[allow(clippy::result_large_err)]
    pub(crate) fn render(&self) -> Result<(), SendError<AppEvent>> {
        if self.needs_render.get() {
            return Ok(());
        }

        self.needs_render.replace(true);
        self.app_event_sender.send(AppEvent::RequestRender)
    }

    pub(crate) fn finish_frame(&mut self) {
        self.needs_render.replace(false);
        self.rendered_frames.add_assign(1);

        let stickers = self.stickers_to_fetch.take();
        if !stickers.is_empty() {
            match self.stickers_supported {
                StickersSupport::Unsupported => {
                    self.stickers_supported = StickersSupport::UnsupportedAndChecked;
                    // Shoot a dummy sticker request to MPD to see what error we get to determine
                    // what exactly is wrong.
                    self.command(|client| {
                        if let Err(err) = client.sticker("", "test") {
                            status_error!("Stickers are not supported by MPD server: '{}'", err);
                        } else {
                            status_error!("Stickers are not supported by MPD server");
                        }
                        Ok(())
                    });
                }
                StickersSupport::UnsupportedAndChecked => {}
                StickersSupport::Supported => {
                    let uris = stickers.into_iter().collect();
                    log::debug!(uris:?; "Fetching stickers after frame");
                    self.query().id(FETCH_SONG_STICKERS).replace_id(FETCH_SONG_STICKERS).query(
                        |client| {
                            let stickers = client.fetch_song_stickers(uris)?;
                            Ok(QueryResult::SongStickers(stickers))
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn query_sync<T: Send + Sync + 'static>(
        &self,
        on_done: impl FnOnce(&mut BackendDispatcher<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = bounded(1);
        let query = QuerySync {
            callback: Box::new(|client| Ok(QueryResult::Any(Box::new((on_done)(client)?)))),
            tx,
        };

        if let Err(err) = self.client_request_sender.send(ClientRequest::QuerySync(query)) {
            log::error!(error:? = err; "Failed to send query request");
            bail!("Failed to send sync query request");
        }

        if let QueryResult::Any(any) = rx.recv()? {
            if let Ok(val) = any.downcast::<T>() {
                return Ok(*val);
            }
            bail!("Received unknown type answer for sync query request",);
        }

        bail!("Received unknown QueryResult for sync query request");
    }

    #[builder(finish_fn(name = query))]
    pub(crate) fn query(
        &self,
        #[builder(finish_fn)] on_done: impl FnOnce(&mut BackendDispatcher<'_>) -> Result<QueryResult>
        + Send
        + 'static,
        id: &'static str,
        target: Option<PaneType>,
        replace_id: Option<&'static str>,
    ) {
        let query = Query { id, target, replace_id, callback: Box::new(on_done) };
        if let Err(err) = self.client_request_sender.send(ClientRequest::Query(query)) {
            log::error!(error:? = err; "Failed to send query request");
        }
    }

    pub(crate) fn command(
        &self,
        callback: impl FnOnce(&mut BackendDispatcher<'_>) -> Result<()> + Send + 'static,
    ) {
        if let Err(err) = self
            .client_request_sender
            .send(ClientRequest::Command(PlayerCommand { callback: Box::new(callback) }))
        {
            log::error!(error:? = err; "Failed to send command request");
        }
    }

    pub(crate) fn find_current_song_in_queue(&self) -> Option<(usize, Song)> {
        if self.status.state == State::Stop {
            return None;
        }

        self.status.songid.and_then(|id| {
            self.queue_store()
                .read()
                .iter()
                .enumerate()
                .find(|(_, song)| song.id == Some(id))
                .map(|(idx, s)| (idx, s.clone()))
        })
    }

    pub(crate) fn find_lrc(&self) -> Result<Option<Lrc>> {
        let Some((_, song)) = self.find_current_song_in_queue() else {
            return Ok(None);
        };

        let Some(lyrics_dir) = &self.config.lyrics_dir else {
            return Ok(None);
        };

        let path = get_lrc_path(lyrics_dir, &song.uri)?;
        log::debug!(path:?; "getting lrc at path");
        match std::fs::read_to_string(&path) {
            Ok(lrc) => return Ok(Some(lrc.parse()?)),
            Err(err) if matches!(err.kind(), std::io::ErrorKind::NotFound) => {
                log::trace!(path:?; "Lyrics not found");
            }
            Err(err) => {
                log::error!(err:?; "Encountered error when searching for sidecar lyrics");
            }
        }

        if let Ok(Some(lrc)) = self.lrc_index.find_lrc_for_song(&song) {
            return Ok(Some(lrc));
        }

        Ok(None)
    }

    pub(crate) fn song_stickers(&self, uri: &str) -> Option<&HashMap<String, String>> {
        if matches!(self.stickers_supported, StickersSupport::UnsupportedAndChecked) {
            return None;
        }
        let stickers = self.stickers.get(uri);

        if stickers.is_none() {
            self.stickers_to_fetch.borrow_mut().insert(uri.to_owned());
        }

        stickers
    }

    pub(crate) fn set_song_stickers(
        &mut self,
        uri: String,
        stickers: HashMap<String, String>,
    ) -> Option<HashMap<String, String>> {
        self.stickers.insert(uri, stickers)
    }

    pub(crate) fn set_stickers(&mut self, stickers: HashMap<String, HashMap<String, String>>) {
        self.stickers = stickers;
    }

    pub(crate) fn stickers(&self) -> &HashMap<String, HashMap<String, String>> {
        &self.stickers
    }

    /// Refresh queue state from backend.
    ///
    /// Use when you need to sync UI with backend queue state.
    pub(crate) fn refresh_queue(&self) {
        log::debug!("Refreshing queue from backend");

        self.query().id("queue_refresh").query(move |client| {
            let queue = client.playlist_info()?;
            Ok(QueryResult::Queue(Some(queue)))
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StickersSupport {
    Supported,
    Unsupported,
    UnsupportedAndChecked,
}

impl From<StickersSupport> for bool {
    fn from(value: StickersSupport) -> Self {
        match value {
            StickersSupport::Supported => true,
            StickersSupport::Unsupported => false,
            StickersSupport::UnsupportedAndChecked => false,
        }
    }
}
