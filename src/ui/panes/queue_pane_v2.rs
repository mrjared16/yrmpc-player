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

use crate::{
    config::keys::{CommonAction, QueueActions},
    ctx::Ctx,
    domain::{ContentType, DetailItem, QueueContent, QueueItemOps, Song},
    shared::{
        events::AppEvent,
        key_event::KeyEvent,
        macros::{modal, status_info},
        mouse_event::MouseEvent,
    },
    ui::{
        UiAppEvent, UiEvent,
        modals::confirm_modal::{Action, ConfirmModal},
        panes::navigator_types::{
            ContentAction, DetailId, EntityRef, InputMode, MoveDirection,
            NavigatorPane, PaneAction, PaneId, TabId, TabPane,
        },
        widgets::content_view::ContentView,
    },
};

use super::Pane;

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
        Self {
            view: ContentView::new(),
            render_mode: QueueRenderMode::Compact,
            queue_version: 0,
        }
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
        // Create QueueContent from ctx.queue
        let current_idx = ctx.find_current_song_in_queue().map(|(idx, _)| idx);
        let content = QueueContent::new(ctx.queue.clone(), current_idx);

        // Replace or push content
        self.view.clear();
        if !ctx.queue.is_empty() {
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
    fn selected_song<'a>(&self, ctx: &'a Ctx) -> Option<&'a Song> {
        self.view.current().and_then(|level| {
            level.section_list.selected_item().and_then(|item| {
                if let DetailItem::Song(song) = item {
                    // Find matching song in ctx.queue by uri
                    ctx.queue.iter().find(|s| s.uri == song.uri)
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
    fn interpret_activation(&self, item: DetailItem, ctx: &Ctx) -> PaneAction {
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
                            let start_index = songs
                                .iter()
                                .position(|s| s.uri == song.uri)
                                .unwrap_or(0);
                            return PaneAction::PlayAll { songs, start_index };
                        }
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
            DetailItem::Header { .. } => PaneAction::Handled,
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
    fn indices_to_ids(&self, indices: &[usize], ctx: &Ctx) -> Vec<u32> {
        indices
            .iter()
            .filter_map(|&idx| ctx.queue.get(idx))
            .filter_map(|s| s.id)
            .collect()
    }

    /// Handle queue-specific actions (delete all, shuffle, etc.)
    fn handle_queue_action(&mut self, action: QueueActions, ctx: &mut Ctx) -> Result<PaneAction> {
        match action {
            QueueActions::Play => {
                if let Some(level) = self.view.current() {
                    if let Some(item) = level.section_list.selected_item() {
                        return Ok(self.interpret_activation(item.clone(), ctx));
                    }
                }
                Ok(PaneAction::Handled)
            }
            QueueActions::Delete => {
                let indices = self.selected_indices();
                let ids = self.indices_to_ids(&indices, ctx);
                if !ids.is_empty() {
                    Ok(PaneAction::QueueDelete(ids))
                } else {
                    Ok(PaneAction::Handled)
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
                                ctx.query()
                                    .id("queue_clear_action")
                                    .query(|client| {
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
                ctx.render()?;
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
        use crate::domain::display::ListItemDisplay;
        use crate::shared::image_cache::ThumbnailSize;
        use crate::ui::widgets::async_image::AsyncImage;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

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
                Layout::vertical([Constraint::Percentage(75), Constraint::Min(4)]).areas::<2>(inner);

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
                let [art_area, list_area] = Layout::horizontal([
                    Constraint::Percentage(35),
                    Constraint::Percentage(65),
                ])
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
        // Sync queue on player events
        if matches!(event, UiEvent::Player) {
            self.sync_queue(ctx);
        }
        Ok(())
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        // Queue-specific actions first
        if let Some(action) = event.as_queue_action(ctx) {
            let _ = self.handle_queue_action(action, ctx)?;
            return Ok(());
        }

        // Delegate to ContentView
        let action = self.view.handle_key(event, ctx);
        match action {
            ContentAction::Activate(item) => {
                let pane_action = self.interpret_activation(item, ctx);
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
                let ids: Vec<u32> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .filter_map(|s| s.id)
                    .collect();
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
            _ => {}
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
            ContentAction::Handled => {
                ctx.render()?;
                Ok(PaneAction::Handled)
            }
            ContentAction::Back => {
                // Clear marks first if any
                if let Some(level) = self.view.current_mut() {
                    if level.section_list.has_marked() {
                        level.section_list.clear_marks();
                        ctx.render()?;
                        return Ok(PaneAction::Handled);
                    }
                }
                Ok(PaneAction::BackPane)
            }
            ContentAction::Activate(item) => {
                let pane_action = self.interpret_activation(item, ctx);
                ctx.render()?;
                Ok(pane_action)
            }
            ContentAction::Mark(_) => {
                ctx.render()?;
                Ok(PaneAction::Handled)
            }
            ContentAction::Delete(items) => {
                let ids: Vec<u32> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    Ok(PaneAction::QueueDelete(ids))
                } else {
                    Ok(PaneAction::Handled)
                }
            }
            ContentAction::MoveUp(items) => {
                let ids: Vec<u32> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    Ok(PaneAction::QueueMove { ids, direction: MoveDirection::Up })
                } else {
                    Ok(PaneAction::Handled)
                }
            }
            ContentAction::MoveDown(items) => {
                let ids: Vec<u32> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    Ok(PaneAction::QueueMove { ids, direction: MoveDirection::Down })
                } else {
                    Ok(PaneAction::Handled)
                }
            }
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
