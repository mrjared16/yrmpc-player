//! Navigator - Central controller for pane navigation.
//!
//! The Navigator manages:
//! - Tab panes (Search, Queue, Library) - always available via hotkeys 1/2/3
//! - Detail panes (Artist, Album, Playlist) - shown when content exists
//! - Pane history for Esc-based navigation
//! - Routing key events to the active pane
//!
//! ## Navigation Flow
//!
//! 1. User presses Enter on an artist in search results
//! 2. SearchPaneV2::handle_key returns PaneAction::NavigateTo(EntityRef {
//!    Artist, id, name })
//! 3. Navigator receives the action, fetches artist content, pushes to
//!    ArtistDetailPane
//! 4. Navigator switches active pane to ArtistDetailPane, adds SearchPane to
//!    history
//! 5. User presses Esc → Navigator pops history, returns to SearchPane
//!
//! ## Coexistence with Legacy
//!
//! The Navigator can coexist with the legacy PaneContainer system.
//! During migration, both systems operate - Navigator manages its panes,
//! legacy system manages others. Full migration replaces PaneContainer.

use std::collections::HashMap;

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use super::{
    UiEvent,
    action_executor::PaneActionExecutor,
    album_detail::AlbumDetailPane,
    artist_detail::ArtistDetailPane,
    library_tab::LibraryTabPane,
    navigator_types::{
        DetailId, DetailPane, EntityContent, EntityRef, InputMode, MoveDirection, NavigatorPane,
        PaneAction, PaneId, TabId, TabPane,
    },
    playlist_detail::PlaylistDetailPane,
    queue_pane_v2::QueuePaneV2,
    search_pane_v2::SearchPaneV2,
};
use crate::{
    actions::{Intent, Selection},
    ctx::Ctx,
    domain::DetailItem,
    shared::key_event::KeyEvent,
};

const NAV_FETCH_PLAYLIST_DETAIL_ID: &str = "navigator_fetch_playlist_detail";
const NAV_FETCH_ALBUM_DETAIL_ID: &str = "navigator_fetch_album_detail";
const NAV_FETCH_ARTIST_DETAIL_ID: &str = "navigator_fetch_artist_detail";

// =============================================================================
// NAVIGATOR
// =============================================================================

/// Central navigator managing pane lifecycle and navigation.
pub struct Navigator {
    // Tab panes (always available)
    search_pane: SearchPaneV2,
    queue_pane: QueuePaneV2,
    library_pane: LibraryTabPane,

    // Detail panes (content stacking)
    artist_pane: ArtistDetailPane,
    album_pane: AlbumDetailPane,
    playlist_pane: PlaylistDetailPane,

    /// Currently active pane
    active: PaneId,

    /// Navigation history (for Esc to go back)
    history: Vec<PaneId>,

    /// Maximum history depth
    max_history: usize,

    /// Action router for Intent dispatch (reused, not recreated)
    action_dispatcher: crate::actions::ActionDispatcher,
}

impl std::fmt::Debug for Navigator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Navigator")
            .field("active", &self.active)
            .field("history", &self.history)
            .field("max_history", &self.max_history)
            .finish_non_exhaustive()
    }
}

impl Navigator {
    /// Create a new Navigator with initialized panes.
    pub fn new(ctx: &Ctx) -> Self {
        Self {
            search_pane: SearchPaneV2::new(ctx),
            queue_pane: QueuePaneV2::new(ctx),
            library_pane: LibraryTabPane::new(ctx),
            artist_pane: ArtistDetailPane::new(),
            album_pane: AlbumDetailPane::new(),
            playlist_pane: PlaylistDetailPane::new(),
            active: PaneId::Tab(TabId::Search),
            history: Vec::new(),
            max_history: 10,
            action_dispatcher: Self::create_action_dispatcher(),
        }
    }

    /// Create the action dispatcher with default handlers.
    fn create_action_dispatcher() -> crate::actions::ActionDispatcher {
        use crate::actions::{
            ActionDispatcher, PlayHandler, QueueHandler, RadioHandler, SaveHandler,
            TogglePlaybackHandler,
        };

        ActionDispatcher::new()
            .with_handler(Box::new(TogglePlaybackHandler::new()))
            .with_handler(Box::new(PlayHandler::new()))
            .with_handler(Box::new(QueueHandler::new()))
            .with_handler(Box::new(RadioHandler::new()))
            .with_handler(Box::new(SaveHandler::new()))
    }

    // =========================================================================
    // PANE ACCESS
    // =========================================================================

    /// Get reference to the currently active pane.
    fn active_pane(&self) -> &dyn NavigatorPane {
        match self.active {
            PaneId::Tab(TabId::Search) => &self.search_pane,
            PaneId::Tab(TabId::Queue) => &self.queue_pane,
            PaneId::Tab(TabId::Library) => &self.library_pane,
            PaneId::Detail(DetailId::Artist) => &self.artist_pane,
            PaneId::Detail(DetailId::Album) => &self.album_pane,
            PaneId::Detail(DetailId::Playlist) => &self.playlist_pane,
        }
    }

    /// Get mutable reference to the currently active pane.
    fn active_pane_mut(&mut self) -> &mut dyn NavigatorPane {
        match self.active {
            PaneId::Tab(TabId::Search) => &mut self.search_pane,
            PaneId::Tab(TabId::Queue) => &mut self.queue_pane,
            PaneId::Tab(TabId::Library) => &mut self.library_pane,
            PaneId::Detail(DetailId::Artist) => &mut self.artist_pane,
            PaneId::Detail(DetailId::Album) => &mut self.album_pane,
            PaneId::Detail(DetailId::Playlist) => &mut self.playlist_pane,
        }
    }

    /// Get mutable reference to a detail pane.
    fn detail_pane_mut(&mut self, id: DetailId) -> &mut dyn DetailPane {
        match id {
            DetailId::Artist => &mut self.artist_pane,
            DetailId::Album => &mut self.album_pane,
            DetailId::Playlist => &mut self.playlist_pane,
        }
    }

    // =========================================================================
    // NAVIGATION
    // =========================================================================

    /// Switch to a specific pane, adding current to history.
    pub fn switch_to(&mut self, pane: PaneId) {
        if self.active == pane {
            return;
        }

        // Don't switch to empty detail panes
        if let PaneId::Detail(detail_id) = pane {
            let has_content = match detail_id {
                DetailId::Artist => self.artist_pane.has_content(),
                DetailId::Album => self.album_pane.has_content(),
                DetailId::Playlist => self.playlist_pane.has_content(),
            };
            if !has_content {
                return;
            }
        }

        // Add current to history
        self.history.push(self.active);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }

        self.active = pane;
    }

    /// Switch to a tab by hotkey (1/2/3).
    pub fn switch_to_tab(&mut self, tab: TabId) {
        self.switch_to(PaneId::Tab(tab));
    }

    /// Go back to previous pane in history.
    pub fn go_back(&mut self) -> bool {
        // Skip empty detail panes in history
        while let Some(prev) = self.history.pop() {
            if let PaneId::Detail(detail_id) = prev {
                let has_content = match detail_id {
                    DetailId::Artist => self.artist_pane.has_content(),
                    DetailId::Album => self.album_pane.has_content(),
                    DetailId::Playlist => self.playlist_pane.has_content(),
                };
                if !has_content {
                    continue; // Skip empty detail pane
                }
            }
            self.active = prev;
            return true;
        }
        false
    }

    /// Navigate to an entity (artist, album, playlist).
    pub fn navigate_to(&mut self, entity: EntityRef, _ctx: &Ctx) {
        log::info!(
            "Navigator::navigate_to: {:?} id={} name={}",
            entity.entity_type,
            entity.id,
            entity.name
        );

        // Switch to the detail pane
        let target = PaneId::Detail(entity.entity_type);

        // Add current to history before switching
        if self.active != target {
            self.history.push(self.active);
            if self.history.len() > self.max_history {
                self.history.remove(0);
            }
        }

        self.active = target;

        // Content should already be pushed via push_content() before this is called.
        // If the detail pane is empty, log a warning.
        let has_content = match entity.entity_type {
            DetailId::Artist => self.artist_pane.has_content(),
            DetailId::Album => self.album_pane.has_content(),
            DetailId::Playlist => self.playlist_pane.has_content(),
        };

        if !has_content {
            log::warn!(
                "Navigator::navigate_to: {:?} pane has no content. \
                 Caller should push_content() before NavigateTo action.",
                entity.entity_type
            );
        }
    }

    /// Request entity details, then route the result into the owning detail pane.
    pub(crate) fn request_navigation(&mut self, entity: EntityRef, ctx: &Ctx) -> Result<()> {
        use crate::{
            QueryResult,
            backends::api::{ContentType, Discovery, Item},
            config::tabs::PaneType,
            domain::content::ContentDetails,
        };

        let (query_id, content_type) = match entity.entity_type {
            DetailId::Artist => (NAV_FETCH_ARTIST_DETAIL_ID, ContentType::Artist),
            DetailId::Album => (NAV_FETCH_ALBUM_DETAIL_ID, ContentType::Album),
            DetailId::Playlist => (NAV_FETCH_PLAYLIST_DETAIL_ID, ContentType::Playlist),
        };

        let item = Item {
            id: entity.id,
            content_type,
            title: entity.name,
            subtitle: None,
            thumbnail: None,
            duration: None,
            queue_id: None,
        };

        ctx.query().id(query_id).replace_id(query_id).target(PaneType::Search).query(
            move |client| match client.details(&item)? {
                ContentDetails::Playlist(details) => Ok(QueryResult::PlaylistDetail(details)),
                ContentDetails::Album(details) => Ok(QueryResult::AlbumDetail(details)),
                ContentDetails::Artist(details) => Ok(QueryResult::ArtistDetail(details)),
                other => {
                    anyhow::bail!("Unexpected detail payload for navigator navigation: {other:?}")
                }
            },
        );

        Ok(())
    }

    /// Push content to a detail pane.
    pub fn push_content(&mut self, content: EntityContent) {
        let detail_id = content.detail_id();
        let pane = self.detail_pane_mut(detail_id);
        pane.push(content);

        // Switch to the pane if not already active
        let target = PaneId::Detail(detail_id);
        if self.active != target {
            self.history.push(self.active);
            if self.history.len() > self.max_history {
                self.history.remove(0);
            }
            self.active = target;
        }
    }

    // =========================================================================
    // KEY HANDLING
    // =========================================================================

    /// Handle a key event, routing to active pane and processing actions.
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        // Route to active pane
        let action = self.active_pane_mut().handle_key(key, ctx)?;

        // Process returned action
        match action {
            PaneAction::Handled => {}
            PaneAction::BackPane => {
                if !self.go_back() {
                    // No history - stay on current pane
                    // Could default to a home pane here
                }
                ctx.render()?;
            }
            PaneAction::NavigateTo(entity) => {
                self.request_navigation(entity, ctx)?;
            }
            PaneAction::Play(song) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                let items = vec![DetailItem::Song(song)];
                executor.execute_intent(ctx, Intent::play(items))?;
            }
            PaneAction::PlayAll { songs, start_index: _ } => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                let items: Vec<DetailItem> = songs.into_iter().map(DetailItem::Song).collect();
                executor.execute_intent(ctx, Intent::play(items))?;
            }
            PaneAction::Enqueue(songs) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                let items: Vec<DetailItem> = songs.into_iter().map(DetailItem::Song).collect();
                executor.execute_intent(ctx, Intent::add_to_queue(items))?;
            }
            PaneAction::QueueDelete(ids) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_queue_delete(ctx, ids)?;
            }
            #[allow(deprecated)]
            PaneAction::QueueMoveUp(ids) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_queue_move(ctx, ids, MoveDirection::Up)?;
            }
            #[allow(deprecated)]
            PaneAction::QueueMoveDown(ids) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_queue_move(ctx, ids, MoveDirection::Down)?;
            }
            PaneAction::QueueMove { ids, direction } => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_queue_move(ctx, ids, direction)?;
            }
            PaneAction::TogglePause => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_intent(ctx, Intent::toggle_playback())?;
            }
            PaneAction::ShowModal(_kind) => {
                // TODO: Implement modal display
                log::info!("Navigator: ShowModal requested");
            }
            PaneAction::Search(query) => {
                // TODO: Implement search action
                log::info!("Navigator: Search requested: {}", query);
            }
            PaneAction::Execute(intent) => {
                let executor = PaneActionExecutor::new(&self.action_dispatcher);
                executor.execute_intent(ctx, intent)?;
            }
        }

        Ok(())
    }

    fn execute_queue_delete(&mut self, ctx: &mut Ctx, ids: Vec<u32>) -> Result<()> {
        ctx.queue_store().remove_ids(&ids);
        ctx.render()?;
        Ok(())
    }

    /// Execute queue move action with unified direction.
    ///
    /// Optimized for the common case where selected items are neighbors
    /// (contiguous block). Instead of moving each item individually, we
    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the currently active pane.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        self.active_pane_mut().render(frame, area, ctx)
    }

    // =========================================================================
    // STATE QUERIES
    // =========================================================================

    /// Get the currently active pane ID.
    pub fn active_pane_id(&self) -> PaneId {
        self.active
    }

    /// Get the current input mode of the active pane.
    pub fn mode(&self) -> InputMode {
        self.active_pane().mode()
    }

    /// Get breadcrumb for display.
    pub fn breadcrumb(&self) -> String {
        match self.active {
            PaneId::Tab(tab) => tab.label().to_string(),
            PaneId::Detail(DetailId::Artist) => {
                if let Some(title) = self.artist_pane.current_title() {
                    format!("Artist > {}", title)
                } else {
                    "Artist".to_string()
                }
            }
            PaneId::Detail(DetailId::Album) => "Album".to_string(),
            PaneId::Detail(DetailId::Playlist) => "Playlist".to_string(),
        }
    }

    /// Get mutable reference to search pane (for external access).
    pub fn search_pane_mut(&mut self) -> &mut SearchPaneV2 {
        &mut self.search_pane
    }

    /// Get mutable reference to queue pane (for external access).
    pub fn queue_pane_mut(&mut self) -> &mut QueuePaneV2 {
        &mut self.queue_pane
    }

    // =========================================================================
    // EVENT HANDLING
    // =========================================================================

    /// Handle UI events (queue changes, playback state, etc.).
    /// Routes events to all panes that might care.
    pub(crate) fn on_event(&mut self, event: &mut UiEvent, ctx: &Ctx) -> Result<()> {
        if let UiEvent::TabChanged(tab_name) = event {
            let tab_id = match tab_name.as_str() {
                "Search" => Some(TabId::Search),
                "Queue" => Some(TabId::Queue),
                "Library" => Some(TabId::Library),
                _ => None,
            };
            if let Some(id) = tab_id {
                self.switch_to_tab(id);
            }
        }

        self.search_pane.on_event(event, ctx)?;
        self.queue_pane.on_event(event, ctx)?;
        self.library_pane.on_event(event, ctx)?;
        self.artist_pane.on_event(event, ctx)?;
        self.album_pane.on_event(event, ctx)?;
        self.playlist_pane.on_event(event, ctx)?;
        Ok(())
    }

    /// Handle async query results. Routes to TARGET pane, not active pane.
    pub(crate) fn on_query_finished(
        &mut self,
        id: &'static str,
        data: crate::QueryResult,
        target: crate::config::tabs::PaneType,
        ctx: &Ctx,
    ) -> Result<()> {
        use crate::config::tabs::PaneType;

        match (id, data, target) {
            (NAV_FETCH_PLAYLIST_DETAIL_ID, crate::QueryResult::PlaylistDetail(details), _) => {
                self.push_content(EntityContent::Playlist(details));
                Ok(())
            }
            (NAV_FETCH_ALBUM_DETAIL_ID, crate::QueryResult::AlbumDetail(details), _) => {
                self.push_content(EntityContent::Album(details));
                Ok(())
            }
            (NAV_FETCH_ARTIST_DETAIL_ID, crate::QueryResult::ArtistDetail(details), _) => {
                self.push_content(EntityContent::Artist(details));
                Ok(())
            }
            (_, data, PaneType::Search) => self.search_pane.on_query_finished(id, data, ctx),
            (_, data, PaneType::Queue) => self.queue_pane.on_query_finished(id, data, ctx),
            _ => Ok(()),
        }
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full tests require Ctx which is complex to mock
    // These are basic structural tests

    #[test]
    fn test_pane_id_equality() {
        assert_eq!(PaneId::Tab(TabId::Search), PaneId::Tab(TabId::Search));
        assert_ne!(PaneId::Tab(TabId::Search), PaneId::Tab(TabId::Queue));
        assert_ne!(PaneId::Tab(TabId::Search), PaneId::Detail(DetailId::Artist));
    }

    #[test]
    fn test_tab_id_labels() {
        assert_eq!(TabId::Search.label(), "Search");
        assert_eq!(TabId::Queue.label(), "Queue");
        assert_eq!(TabId::Library.label(), "Library");
    }

    #[test]
    fn test_tab_id_hotkeys() {
        assert_eq!(TabId::Search.hotkey(), '1');
        assert_eq!(TabId::Queue.hotkey(), '2');
        assert_eq!(TabId::Library.hotkey(), '3');
    }

    // =========================================================================
    // RED TEST: Task-51 - Number key should update ctx.active_tab
    // =========================================================================
    //
    // This test verifies that pressing number keys (1/2/3) in Navigator
    // should update BOTH:
    // 1. Navigator's internal active pane (works correctly)
    // 2. ctx.active_tab (DOES NOT WORK - this is the bug)
    //
    // Currently, Navigator::handle_key() calls switch_to_tab() which only
    // updates self.active but never touches ctx.active_tab. The tab bar
    // reads ctx.active_tab for highlighting, so they get out of sync.

    use std::{
        cell::{Cell, RefCell},
        collections::{HashMap, HashSet},
        sync::{Arc, RwLock},
    };

    use crossbeam::channel::unbounded;
    use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

    use crate::{
        config::Config,
        ctx::Ctx,
        domain::Status,
        mpd::version::Version,
        shared::{image_cache::ImageCache, ring_vec::RingVec},
    };

    fn create_test_ctx() -> Ctx {
        let (tx, _rx) = unbounded();
        let (work_tx, _work_rx) = unbounded();
        let (client_tx, _client_rx) = unbounded();

        let key_config_file = crate::config::keys::KeyConfigFile::default();
        let key_config: crate::config::keys::KeyConfig = key_config_file.try_into().unwrap();
        let config = Config::default();
        let config_with_keybinds = Config { keybinds: key_config, ..config };

        Ctx {
            backend_version: Version::new(0, 0, 0),
            config: Arc::new(config_with_keybinds),
            status: Status::default(),
            image_cache: ImageCache::new(tx.clone()),
            app_state: Arc::new(RwLock::new(crate::app_state::AppState::default())),
            controllers: crate::core::controllers::Controllers::new(
                vec![],
                tx.clone(),
                client_tx.clone(),
            ),
            stickers: HashMap::new(),
            // Start with Search tab active
            active_tab: crate::config::tabs::TabName::from("Search"),
            supported_commands: HashSet::new(),
            capabilities: &[],
            db_update_start: None,
            app_event_sender: tx.clone(),
            work_sender: work_tx,
            client_request_sender: client_tx.clone(),
            needs_render: Cell::new(false),
            stickers_to_fetch: RefCell::new(HashSet::new()),
            lrc_index: Default::default(),
            rendered_frames: 0,
            messages: RingVec::default(),
            last_status_update: std::time::Instant::now(),
            song_played: None,
            stickers_supported: crate::ctx::StickersSupport::Unsupported,
            scheduler: crate::core::scheduler::Scheduler::new((tx, client_tx)),
            debug_ui_log: None,
            queue_panel_visible: false,
            previous_tab: None,
        }
    }

    /// Test: Navigator syncs with ctx.active_tab via UiEvent::TabChanged
    ///
    /// Flow: Ui::change_tab() sets ctx.active_tab then fires
    /// UiEvent::TabChanged. Navigator's on_event() receives the event and
    /// syncs self.active.
    #[test]
    fn navigator_syncs_via_tab_changed_event() {
        let mut ctx = create_test_ctx();

        assert_eq!(ctx.active_tab.as_str(), "Search", "Initial state should be Search tab");

        let mut navigator = Navigator::new(&ctx);
        assert_eq!(
            navigator.active,
            PaneId::Tab(TabId::Search),
            "Navigator should start on Search"
        );

        ctx.active_tab = crate::config::tabs::TabName::from("Queue");
        let mut event = UiEvent::TabChanged(crate::config::tabs::TabName::from("Queue"));
        let _ = navigator.on_event(&mut event, &ctx);

        assert_eq!(
            navigator.active,
            PaneId::Tab(TabId::Queue),
            "Navigator should sync to Queue via TabChanged event"
        );
    }

    fn make_test_artist() -> crate::domain::ArtistContent {
        crate::domain::ArtistContent {
            id: "artist123".to_string(),
            name: "Test Artist".to_string(),
            top_songs: vec![],
            thumbnail: None,
            bio: None,
            extensions: crate::domain::content::Extensions::default(),
        }
    }

    fn make_test_playlist() -> crate::domain::PlaylistContent {
        crate::domain::PlaylistContent {
            id: "playlist123".to_string(),
            title: "Test Playlist".to_string(),
            tracks: vec![],
            author: None,
            thumbnail: None,
            description: None,
            track_count: None,
            duration_text: None,
            extensions: crate::domain::content::Extensions::default(),
        }
    }

    #[test]
    fn navigator_routes_artist_detail_results_into_artist_pane() {
        let ctx = create_test_ctx();
        let mut navigator = Navigator::new(&ctx);

        navigator
            .on_query_finished(
                NAV_FETCH_ARTIST_DETAIL_ID,
                crate::QueryResult::ArtistDetail(make_test_artist()),
                crate::config::tabs::PaneType::Search,
                &ctx,
            )
            .unwrap();

        assert_eq!(navigator.active, PaneId::Detail(DetailId::Artist));
        assert!(navigator.artist_pane.has_content());
        assert_eq!(navigator.artist_pane.current_title(), Some("Test Artist"));
        assert_eq!(navigator.history, vec![PaneId::Tab(TabId::Search)]);
    }

    #[test]
    fn navigator_routes_playlist_detail_results_into_playlist_pane() {
        let ctx = create_test_ctx();
        let mut navigator = Navigator::new(&ctx);

        navigator.switch_to_tab(TabId::Library);

        navigator
            .on_query_finished(
                NAV_FETCH_PLAYLIST_DETAIL_ID,
                crate::QueryResult::PlaylistDetail(make_test_playlist()),
                crate::config::tabs::PaneType::Search,
                &ctx,
            )
            .unwrap();

        assert_eq!(navigator.active, PaneId::Detail(DetailId::Playlist));
        assert!(navigator.playlist_pane.has_content());
        assert_eq!(navigator.playlist_pane.current_title(), Some("Test Playlist"));
        assert_eq!(
            navigator.history,
            vec![PaneId::Tab(TabId::Search), PaneId::Tab(TabId::Library)]
        );
    }
}
