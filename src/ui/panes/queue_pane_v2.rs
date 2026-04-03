//! Queue Pane V2 - Uses ContentView for unified UI architecture
//!
//! This pane uses ContentView<QueueContent> for consistent layered architecture
//! with section headers ("Now Playing", "Up Next").
//!
//! ## Architecture
//!
//! ```text
//! QueuePaneV2
//!   └── ContentView<QueueContent>
//!         └── SectionList
//!               ├── "Now Playing" section (current song)
//!               └── "Up Next" section (remaining songs)
//! ```

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders},
};

use super::Pane;
use crate::{
    config::keys::{CommonAction, QueueActions},
    ctx::Ctx,
    domain::{ContentType, DetailItem, QueueContent, QueueItemOps, Song},
    shared::{
        events::AppEvent,
        key_event::KeyEvent,
        macros::{modal, status_error, status_info},
        mouse_event::MouseEvent,
    },
    ui::{
        UiAppEvent, UiEvent,
        modals::confirm_modal::{Action, ConfirmModal},
        panes::navigator_types::{
            ContentAction, DetailId, EntityRef, InputMode, MoveDirection, NavigatorPane,
            PaneAction, PaneId, TabId, TabPane,
        },
        widgets::content_view::ContentView,
    },
};

/// Render mode for the queue pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueRenderMode {
    /// Compact list only
    #[default]
    Compact,
    /// Player mode: large album art + queue list
    Player,
}

/// Queue Pane V2 - uses ContentView for unified architecture
#[derive(Debug, Default)]
pub struct QueuePaneV2 {
    /// ContentView handles all stack management and key handling
    view: ContentView<QueueContent>,
    /// Render mode (compact vs player)
    render_mode: QueueRenderMode,
    /// Cached queue version to detect changes
    queue_version: usize,
}

impl QueuePaneV2 {
    pub fn new(_ctx: &Ctx) -> Self {
        Self { view: ContentView::new(), render_mode: QueueRenderMode::Compact, queue_version: 0 }
    }

    /// Toggle between Compact and Player render modes
    pub fn toggle_render_mode(&mut self) {
        self.render_mode = match self.render_mode {
            QueueRenderMode::Compact => QueueRenderMode::Player,
            QueueRenderMode::Player => QueueRenderMode::Compact,
        };
    }

    /// Sync ContentView with current queue state from Ctx
    fn sync_queue(&mut self, ctx: &Ctx) {
        let current_idx = ctx.find_current_song_in_queue().map(|(idx, _)| idx);
        let queue = ctx.queue_store().read();
        let content = QueueContent::new(queue.clone(), current_idx);

        self.view.clear();
        if !queue.is_empty() {
            self.view.push(content);
        }
    }

    /// Jump selection to currently playing song
    fn jump_to_current(&mut self, ctx: &Ctx) {
        if let Some(level) = self.view.current_mut() {
            if let Some((idx, _)) = ctx.find_current_song_in_queue() {
                level.section_list.list_view_mut().select(Some(idx));
            } else {
                status_info!("No song is currently playing");
            }
        }
    }

    /// Get the currently selected song (if any)
    fn selected_song<'a>(&self, ctx: &'a Ctx) -> Option<Song> {
        self.view.current().and_then(|level| {
            level.section_list.selected_item().and_then(|item| {
                if let DetailItem::Song(song) = item {
                    let queue = ctx.queue_store().read();
                    queue.iter().find(|s| s.uri == song.uri).cloned()
                } else {
                    None
                }
            })
        })
    }

    /// Navigate to artist details for the selected queue item
    fn navigate_to_artist(&self, ctx: &Ctx) -> Option<EntityRef> {
        let song = self.selected_song(ctx)?;

        // Try to get artist browse ID from metadata
        if let Some(artist_id) = song.metadata.get("artist_browse_id").and_then(|v| v.first()) {
            let artist_name = song.artist().unwrap_or("Artist").to_string();
            Some(EntityRef {
                entity_type: DetailId::Artist,
                id: artist_id.clone(),
                name: artist_name,
            })
        } else {
            if let Some(artist_name) = song.artist() {
                log::warn!("No artist_browse_id for '{}', navigation not available", artist_name);
                status_info!("Artist navigation not available for this track");
            }
            None
        }
    }

    /// Interpret what activation means for a DetailItem in QueuePane
    fn resolve_action(&self, item: DetailItem, ctx: &Ctx) -> PaneAction {
        match item {
            DetailItem::Song(song) => {
                // Check for marked items
                if let Some(level) = self.view.current() {
                    if level.section_list.has_marked() {
                        let songs: Vec<Song> = level
                            .section_list
                            .marked_items()
                            .iter()
                            .filter_map(|i| i.as_song().cloned())
                            .collect();
                        if !songs.is_empty() {
                            let start_index =
                                songs.iter().position(|s| s.uri == song.uri).unwrap_or(0);
                            return PaneAction::PlayAll { songs, start_index };
                        }
                    }
                }

                // Check if this song is currently playing
                if let Some((_, current_song)) = ctx.find_current_song_in_queue() {
                    if current_song.uri == song.uri
                        && (ctx.status.state == crate::domain::PlaybackState::Play
                            || ctx.status.state == crate::domain::PlaybackState::Pause)
                    {
                        return PaneAction::TogglePause;
                    }
                }

                PaneAction::Play(song)
            }
            DetailItem::Ref(content_ref) => {
                let entity_type = match content_ref.content_type {
                    ContentType::Artist => DetailId::Artist,
                    ContentType::Album => DetailId::Album,
                    ContentType::Playlist => DetailId::Playlist,
                    _ => return PaneAction::Handled,
                };
                PaneAction::NavigateTo(EntityRef {
                    entity_type,
                    id: content_ref.id,
                    name: content_ref.name,
                })
            }
        }
    }

    /// Get selected indices for queue operations
    fn selected_indices(&self) -> Vec<usize> {
        if let Some(level) = self.view.current() {
            if level.section_list.has_marked() {
                // Return marked item indices
                level.section_list.marked_indices().collect()
            } else if let Some(idx) = level.section_list.selected() {
                vec![idx]
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    }

    /// Convert indices to queue IDs
    fn indices_to_ids(&self, indices: &[usize], ctx: &Ctx) -> Result<Vec<u32>> {
        let queue = ctx.queue_store().read();
        let mut ids = Vec::new();
        for &idx in indices {
            match queue.get(idx).and_then(|s| s.id) {
                Some(id) => ids.push(id),
                None => anyhow::bail!("Selected items not found in queue"),
            }
        }
        Ok(ids)
    }

    /// Handle queue-specific actions (delete all, shuffle, etc.)
    fn handle_queue_action(&mut self, action: QueueActions, ctx: &mut Ctx) -> Result<PaneAction> {
        match action {
            QueueActions::Play => {
                if let Some(level) = self.view.current() {
                    if let Some(item) = level.section_list.selected_item() {
                        return Ok(self.resolve_action(item.clone(), ctx));
                    }
                }
                Ok(PaneAction::Handled)
            }
            QueueActions::Delete => {
                let indices = self.selected_indices();
                match self.indices_to_ids(&indices, ctx) {
                    Ok(ids) if !ids.is_empty() => Ok(PaneAction::QueueDelete(ids)),
                    Ok(_) => Ok(PaneAction::Handled),
                    Err(e) => {
                        status_error!("{}", e);
                        Ok(PaneAction::Handled)
                    }
                }
            }
            QueueActions::DeleteAll => {
                modal!(
                    ctx,
                    ConfirmModal::builder()
                        .ctx(ctx)
                        .message(vec![
                            "Are you sure you want to clear the queue?",
                            "This action cannot be undone."
                        ])
                        .action(Action::Single {
                            on_confirm: Box::new(|ctx| {
                                ctx.query().id("queue_clear_action").query(|client| {
                                    client.clear()?;
                                    let queue = client.playlist_info()?;
                                    Ok(crate::QueryResult::Queue(Some(queue)))
                                });
                                Ok(())
                            }),
                            confirm_label: Some("Clear"),
                            cancel_label: None,
                        })
                        .size((45, 6))
                        .build()
                );
                Ok(PaneAction::Handled)
            }
            QueueActions::JumpToCurrent => {
                self.jump_to_current(ctx);
                Ok(PaneAction::Handled)
            }
            QueueActions::Shuffle => {
                ctx.command(move |client| {
                    client.shuffle(None)?;
                    Ok(())
                });
                status_info!("Shuffled the queue");
                Ok(PaneAction::Handled)
            }
            _ => Ok(PaneAction::Handled),
        }
    }

    /// Render the now playing section with large album art (Player mode)
    fn render_now_playing(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        use ratatui::{
            text::{Line, Span},
            widgets::Paragraph,
        };

        use crate::{
            domain::display::ListItemDisplay, shared::image_cache::ThumbnailSize,
            ui::widgets::async_image::AsyncImage,
        };

        let config = &ctx.config;

        let block = Block::default()
            .title(" Now Playing ")
            .borders(Borders::ALL)
            .border_style(config.as_border_style());

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Get current song
        if let Some((_, song)) = ctx.find_current_song_in_queue() {
            // Split for album art (top) and info (bottom)
            let [img_area, info_area] =
                Layout::vertical([Constraint::Percentage(75), Constraint::Min(4)])
                    .areas::<2>(inner);

            // Render large album art
            let thumbnail_url = song.thumbnail_url().map(|s| s.to_string());
            let image =
                AsyncImage::new(&ctx.image_cache, thumbnail_url).size(ThumbnailSize::AlbumArt);
            frame.render_widget(image, img_area);

            // Render song info
            let title = song.title();
            let artist = song.artist().unwrap_or("Unknown Artist");
            let album = song.album().unwrap_or("");

            let info = Paragraph::new(vec![
                Line::from(Span::styled(title, config.theme.current_item_style)),
                Line::from(Span::styled(artist, config.as_text_style())),
                Line::from(Span::styled(album, config.as_text_style())),
            ]);

            frame.render_widget(info, info_area);
        }
    }
}

// =============================================================================
// LEGACY PANE TRAIT IMPLEMENTATION
// =============================================================================

impl Pane for QueuePaneV2 {
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        // Sync queue before rendering
        self.sync_queue(ctx);

        match self.render_mode {
            QueueRenderMode::Compact => {
                self.view.render(frame, area, ctx);
            }
            QueueRenderMode::Player => {
                // Split: 35% album art, 65% queue list
                let [art_area, list_area] =
                    Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                        .areas::<2>(area);

                // Render now playing with album art
                self.render_now_playing(frame, art_area, ctx);

                // Render queue list using ContentView
                self.view.render(frame, list_area, ctx);
            }
        }
        Ok(())
    }

    fn on_event(&mut self, event: &mut UiEvent, _is_visible: bool, ctx: &Ctx) -> Result<()> {
        if matches!(event, UiEvent::Player | UiEvent::QueueChanged) {
            self.sync_queue(ctx);
        }
        Ok(())
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        // Queue-specific actions first
        if let Some(action) = event.as_queue_action(ctx) {
            let pane_action = self.handle_queue_action(action, ctx)?;
            match pane_action {
                PaneAction::Handled => ctx.render()?,
                PaneAction::Play(song) => {
                    if let Some(id) = song.id {
                        ctx.command(move |client| {
                            client.play_id(id)?;
                            Ok(())
                        });
                    }
                }
                PaneAction::QueueDelete(ids) => {
                    for id in ids {
                        ctx.command(move |client| {
                            client.delete_id(id)?;
                            Ok(())
                        });
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // Delegate to ContentView
        let action = self.view.handle_key(event, ctx);
        match action {
            ContentAction::Handled => {
                // Handled internally by ContentView
            }
            ContentAction::Back => {
                // Back at root - no-op in queue pane
            }
            ContentAction::Mark(_) => {
                // Marks handled internally by SectionList
            }
            ContentAction::PlayScope(_) => {}
            ContentAction::Activate(item) => {
                let pane_action = self.resolve_action(item, ctx);
                // For legacy Pane, we just trigger the action directly
                if let PaneAction::Play(song) = pane_action {
                    if let Some(id) = song.id {
                        ctx.command(move |client| {
                            client.play_id(id)?;
                            Ok(())
                        });
                    }
                }
            }
            ContentAction::Delete(items) => {
                let ids: Vec<u32> =
                    items.iter().filter_map(|i| i.as_song()).filter_map(|s| s.id).collect();
                if !ids.is_empty() {
                    for id in ids {
                        ctx.command(move |client| {
                            client.delete_id(id)?;
                            Ok(())
                        });
                    }
                }
            }
            ContentAction::MoveUp(items) | ContentAction::MoveDown(items) => {
                // Queue move operations - would need queue IDs
                // For now, just render
            }
            ContentAction::Enqueue(_) => {
                // Items already in queue - enqueue is a no-op
            }
            ContentAction::Passthrough => {}
        }
        ctx.render()?;
        Ok(())
    }

    fn handle_mouse_event(&mut self, _event: MouseEvent, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }
}

// =============================================================================
// NEW ARCHITECTURE: NavigatorPane + TabPane Implementation
// =============================================================================

impl NavigatorPane for QueuePaneV2 {
    fn id(&self) -> PaneId {
        PaneId::Tab(TabId::Queue)
    }

    fn mode(&self) -> InputMode {
        self.view.mode()
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        Pane::render(self, frame, area, ctx)
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        // Queue-specific actions first
        if let Some(action) = key.as_queue_action(ctx) {
            return self.handle_queue_action(action, ctx);
        }

        // Sync queue state
        self.sync_queue(ctx);

        // Delegate to ContentView
        let action = self.view.handle_key(key, ctx);

        match action {
            ContentAction::Handled => Ok(PaneAction::Handled),
            ContentAction::Back => {
                // Clear marks first if any
                if let Some(level) = self.view.current_mut() {
                    if level.section_list.has_marked() {
                        level.section_list.clear_marks();
                        return Ok(PaneAction::Handled);
                    }
                }
                Ok(PaneAction::BackPane)
            }
            ContentAction::Activate(item) => {
                let pane_action = self.resolve_action(item, ctx);
                Ok(pane_action)
            }
            ContentAction::PlayScope(_) => Ok(PaneAction::Handled),
            ContentAction::Mark(_) => Ok(PaneAction::Handled),
            ContentAction::Delete(items) => {
                let ids: Vec<u32> =
                    items.iter().filter_map(|i| i.as_song()).filter_map(|s| s.id).collect();
                if !ids.is_empty() {
                    Ok(PaneAction::QueueDelete(ids))
                } else {
                    Ok(PaneAction::Handled)
                }
            }
            ContentAction::MoveUp(items) => {
                let ids: Vec<u32> =
                    items.iter().filter_map(|i| i.as_song()).filter_map(|s| s.id).collect();
                if !ids.is_empty() {
                    Ok(PaneAction::QueueMove { ids, direction: MoveDirection::Up })
                } else {
                    Ok(PaneAction::Handled)
                }
            }
            ContentAction::MoveDown(items) => {
                let ids: Vec<u32> =
                    items.iter().filter_map(|i| i.as_song()).filter_map(|s| s.id).collect();
                if !ids.is_empty() {
                    Ok(PaneAction::QueueMove { ids, direction: MoveDirection::Down })
                } else {
                    Ok(PaneAction::Handled)
                }
            }
            ContentAction::Enqueue(_) => {
                // Items already in queue - enqueue is a no-op
                Ok(PaneAction::Handled)
            }
            ContentAction::Passthrough => Ok(PaneAction::Handled),
        }
    }

    fn on_event(&mut self, event: &mut UiEvent, ctx: &Ctx) -> Result<()> {
        // Sync queue on player/queue events
        if matches!(event, UiEvent::Player) {
            self.sync_queue(ctx);
        }
        Ok(())
    }
}

impl TabPane for QueuePaneV2 {
    fn tab_id(&self) -> TabId {
        TabId::Queue
    }

    fn current_stage(&self) -> &str {
        "List" // Queue has single stage
    }

    fn can_go_back_stage(&self) -> bool {
        false // No internal stages
    }

    fn go_back_stage(&mut self) -> bool {
        false // Nothing to go back to
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::{HashMap, HashSet},
        sync::{Arc, RwLock},
    };

    use crossbeam::channel::unbounded;

    use super::*;
    use crate::{
        config::Config,
        ctx::Ctx,
        domain::{PlaybackState, Song, Status},
        mpd::version::Version,
        shared::{image_cache::ImageCache, ring_vec::RingVec},
    };

    fn create_test_ctx() -> Ctx {
        let (tx, _rx) = unbounded();
        let (work_tx, _work_rx) = unbounded();
        let (client_tx, _client_rx) = unbounded();

        Ctx {
            backend_version: Version::new(0, 0, 0),
            config: Arc::new(Config::default()),
            status: Status::default(),
            image_cache: ImageCache::new(tx.clone()),
            app_state: Arc::new(RwLock::new(crate::app_state::AppState::default())),
            controllers: crate::core::controllers::Controllers::new(
                vec![],
                tx.clone(),
                client_tx.clone(),
            ),
            stickers: HashMap::new(),
            active_tab: crate::config::tabs::TabName::from("Queue"),
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

    #[test]
    fn resolve_action_toggles_pause_for_playing_song() {
        let mut ctx = create_test_ctx();
        let pane = QueuePaneV2::new(&ctx);

        let song = Song { id: Some(1), uri: "test_uri".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song.clone()]);
        ctx.status.songid = Some(1);
        ctx.status.state = PlaybackState::Play;

        let item = DetailItem::Song(song);
        let action = pane.resolve_action(item, &ctx);

        match action {
            PaneAction::TogglePause => {}
            PaneAction::Play(_) => panic!("Should toggle pause, not play"),
            _ => panic!("Unexpected action: {:?}", action),
        }
    }

    #[test]
    fn resolve_action_plays_different_song() {
        let mut ctx = create_test_ctx();
        let pane = QueuePaneV2::new(&ctx);

        let song1 = Song { id: Some(1), uri: "uri1".to_string(), ..Default::default() };
        let song2 = Song { id: Some(2), uri: "uri2".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song1.clone(), song2.clone()]);

        ctx.status.songid = Some(1);
        ctx.status.state = PlaybackState::Play;

        let item = DetailItem::Song(song2.clone());
        let action = pane.resolve_action(item, &ctx);

        match action {
            PaneAction::Play(s) => assert_eq!(s.id, song2.id),
            _ => panic!("Should play song2"),
        }
    }

    #[test]
    fn indices_to_ids_errors_on_missing_ids() {
        let ctx = create_test_ctx();
        let pane = QueuePaneV2::new(&ctx);

        let song = Song { id: Some(100), uri: "uri".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song]);

        let indices = vec![0, 1];
        let result = pane.indices_to_ids(&indices, &ctx);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Selected items not found in queue");
    }

    #[test]
    fn resolve_action_resumes_paused_song() {
        let mut ctx = create_test_ctx();
        let pane = QueuePaneV2::new(&ctx);

        let song = Song { id: Some(1), uri: "test_uri".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song.clone()]);
        ctx.status.songid = Some(1);
        ctx.status.state = PlaybackState::Pause;

        let item = DetailItem::Song(song);
        let action = pane.resolve_action(item, &ctx);

        match action {
            PaneAction::TogglePause => {}
            _ => panic!("Should toggle pause (resume), got: {:?}", action),
        }
    }
}
