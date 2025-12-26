//! Queue Pane V2 - Clean rewrite using InteractiveListView and QueueItemOps
//!
//! Uses the generic InteractiveListView for navigation/rendering and
//! handles Queue-specific actions directly in the pane.
//!
//! ## New Architecture Integration
//!
//! This pane implements both:
//! - Legacy `Pane` trait (for current UI system)
//! - New `NavigatorPane` + `TabPane` traits (for Navigator system)
//!
//! The new traits delegate to existing methods, ensuring no functionality loss.

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
    domain::{ContentType, QueueItemAction, QueueItemOps, Song, PlaybackState},
    mpd::commands::SeekPosition,
    shared::{
        events::AppEvent,
        key_event::KeyEvent,
        macros::{modal, status_error, status_info},
        mouse_event::MouseEvent,
    },
    ui::{
        UiEvent,
        UiAppEvent,
        list_ops::{self, MoveDirection, QueueListBehavior},
        modals::confirm_modal::{Action, ConfirmModal},
        panes::navigator_types::{
            DetailId, EntityRef, InputMode, ListAction,
            NavigatorPane, PaneAction, PaneId, TabId, TabPane,
        },
        widgets::interactive_list_view::{InteractiveListView, NavConfig},
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

/// Queue Pane V2 - uses InteractiveListView for rendering, handles QueueItemOps directly
#[derive(Debug)]
pub struct QueuePaneV2 {
    list_view: InteractiveListView,
    render_mode: QueueRenderMode,
}

impl QueuePaneV2 {
    pub fn new(_ctx: &Ctx) -> Self {
        Self {
            list_view: InteractiveListView::new(),
            render_mode: QueueRenderMode::Compact,
        }
    }

    /// Toggle between Compact and Player render modes
    pub fn toggle_render_mode(&mut self) {
        self.render_mode = match self.render_mode {
            QueueRenderMode::Compact => QueueRenderMode::Player,
            QueueRenderMode::Player => QueueRenderMode::Compact,
        };
    }

    /// Jump selection to currently playing song
    fn jump_to_current(&mut self, ctx: &Ctx) {
        if let Some((idx, _)) = ctx.find_current_song_in_queue() {
            self.list_view.select(Some(idx));
        } else {
            status_info!("No song is currently playing");
        }
    }

    /// Get the currently selected song (if any)
    fn selected_song<'a>(&self, ctx: &'a Ctx) -> Option<&'a Song> {
        self.list_view.selected().and_then(|idx| ctx.queue.get(idx))
    }

    /// Navigate to artist details for the selected queue item.
    fn navigate_to_artist(&self, ctx: &Ctx) {
        let Some(song) = self.selected_song(ctx) else {
            return;
        };

        // Try to get artist browse ID from metadata
        if let Some(artist_id) = song.metadata.get("artist_browse_id").and_then(|v| v.first()) {
            log::info!("Navigating to artist ID: {}", artist_id);
            let artist_name = song.artist().unwrap_or("Artist").to_string();
            let _ = ctx.app_event_sender.send(AppEvent::UiEvent(UiAppEvent::NavigateTo {
                id: artist_id.clone(),
                kind: ContentType::Artist,
                title: Some(artist_name),
            }));
        } else if let Some(artist_name) = song.artist() {
            log::warn!("No artist_browse_id for '{}', navigation not available", artist_name);
            status_info!("Artist navigation not available for this track");
        }
    }
}

// Implement QueueListBehavior trait for shared action logic
impl QueueListBehavior for QueuePaneV2 {
    fn list_view(&self) -> &InteractiveListView {
        &self.list_view
    }

    fn list_view_mut(&mut self) -> &mut InteractiveListView {
        &mut self.list_view
    }
    // Uses default implementations for play_selected, delete_selected, move_selected
}


impl Pane for QueuePaneV2 {
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        // Find current playing song for highlight
        let current_song_id = ctx.find_current_song_in_queue().map(|(_, song)| song.id);

        match self.render_mode {
            QueueRenderMode::Compact => {
                // Use InteractiveListView for queue items
                self.list_view.render(
                    frame,
                    area,
                    ctx,
                    &ctx.queue,
                    Some("Queue"),
                    |_idx, song| current_song_id.is_some_and(|id| id == song.id),
                );
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

                // Render queue list
                self.list_view.render(
                    frame,
                    list_area,
                    ctx,
                    &ctx.queue,
                    Some("Queue"),
                    |_idx, song| current_song_id.is_some_and(|id| id == song.id),
                );
            }
        }
        Ok(())
    }

    fn on_event(&mut self, event: &mut UiEvent, _is_visible: bool, ctx: &Ctx) -> Result<()> {
        // Handle queue changes - validate selection
        if let UiEvent::Player = event {
            let len = ctx.queue.len();
            if let Some(idx) = self.list_view.selected() {
                if idx >= len {
                    self.list_view.select(if len > 0 { Some(len - 1) } else { None });
                }
            }
        }
        
        // Sync to current playing song
        if let Some((idx, _)) = ctx.find_current_song_in_queue() {
            self.list_view.sync_to(Some(idx));
        }
        Ok(())
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        // Queue-specific actions first
        if let Some(action) = event.as_queue_action(ctx) {
            match action {
                QueueActions::Play => {
                    QueueListBehavior::play_selected(self, ctx);
                    ctx.render()?;
                }
                QueueActions::Delete => {
                    QueueListBehavior::delete_selected(self, ctx);
                    ctx.render()?;
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
                                    // Use query (not command) to trigger UI refresh
                                    ctx.query()
                                        .id("queue_clear_action")
                                        .query(|client| {
                                            client.clear()?;
                                            // Return empty queue for instant UI refresh
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
                }
                QueueActions::JumpToCurrent => {
                    self.jump_to_current(ctx);
                    ctx.render()?;
                }
                QueueActions::Shuffle => {
                    ctx.command(move |client| {
                        client.shuffle(None)?;
                        Ok(())
                    });
                    status_info!("Shuffled the queue");
                }
                _ => {}
            }
            return Ok(());
        }

        // Common navigation actions
        if let Some(action) = event.as_common_action(ctx) {
            match action {
                CommonAction::Up => {
                    self.list_view.select_prev(&ctx.queue, NavConfig::default());
                    ctx.render()?;
                }
                CommonAction::Down => {
                    self.list_view.select_next(&ctx.queue, NavConfig::default());
                    ctx.render()?;
                }
                CommonAction::Confirm => {
                    QueueListBehavior::play_selected(self, ctx);
                    ctx.render()?;
                }
                CommonAction::Delete => {
                    QueueListBehavior::delete_selected(self, ctx);
                    ctx.render()?;
                }
                CommonAction::Top => {
                    self.list_view.select_first(&ctx.queue);
                    ctx.render()?;
                }
                CommonAction::Bottom => {
                    self.list_view.select_last(&ctx.queue);
                    ctx.render()?;
                }
                CommonAction::MoveUp => {
                    QueueListBehavior::move_selected(self, MoveDirection::Up, ctx);
                    ctx.render()?;
                }
                CommonAction::MoveDown => {
                    QueueListBehavior::move_selected(self, MoveDirection::Down, ctx);
                    ctx.render()?;
                }
                CommonAction::Right => {
                    // Navigate to artist details
                    self.navigate_to_artist(ctx);
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn handle_mouse_event(&mut self, _event: MouseEvent, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }
}

impl QueuePaneV2 {
    /// Render the now playing section with large album art (Player mode)
    fn render_now_playing(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        use crate::ui::widgets::async_image::AsyncImage;
        use crate::shared::image_cache::ThumbnailSize;
        use crate::domain::display::ListItemDisplay;
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
            let [img_area, info_area] = Layout::vertical([
                Constraint::Percentage(75),  // Most space for image
                Constraint::Min(4),          // Minimum for info
            ])
            .areas::<2>(inner);

            // Render large album art
            let thumbnail_url = song.thumbnail_url().map(|s| s.to_string());
            let image = AsyncImage::new(&ctx.image_cache, thumbnail_url)
                .size(ThumbnailSize::AlbumArt);
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
// NEW ARCHITECTURE: NavigatorPane + TabPane Implementation
// =============================================================================
//
// These implementations allow QueuePaneV2 to work with the new Navigator system
// while preserving ALL existing functionality from the legacy Pane trait.

impl NavigatorPane for QueuePaneV2 {
    fn id(&self) -> PaneId {
        PaneId::Tab(TabId::Queue)
    }

    fn mode(&self) -> InputMode {
        // Derive mode from list_view state
        if self.list_view.is_find_mode() {
            InputMode::Find
        } else {
            InputMode::Normal
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        // Delegate to existing Pane::render implementation
        Pane::render(self, frame, area, ctx)
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        let queue = &ctx.queue;

        // Use unified handle_key from InteractiveListView
        let list_action = self.list_view.handle_key(key, queue, ctx);

        // Translate ListAction to PaneAction
        match list_action {
            ListAction::Handled => {
                ctx.render()?;
                return Ok(PaneAction::Handled);
            }

            ListAction::Activate(idx) => {
                // Play the selected song
                if let Some(song) = ctx.queue.get(idx).cloned() {
                    QueueListBehavior::play_selected(self, ctx);
                    ctx.render()?;
                    return Ok(PaneAction::Play(song));
                }
                return Ok(PaneAction::Handled);
            }

            ListAction::Mark(_) => {
                ctx.render()?;
                return Ok(PaneAction::Handled);
            }

            ListAction::Delete(indices) => {
                // Convert indices to queue IDs and delete
                let ids: Vec<u32> = indices
                    .iter()
                    .filter_map(|&idx| ctx.queue.get(idx))
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    return Ok(PaneAction::QueueDelete(ids));
                }
                return Ok(PaneAction::Handled);
            }

            ListAction::MoveUp(indices) => {
                // Convert indices to queue IDs and move up
                let ids: Vec<u32> = indices
                    .iter()
                    .filter_map(|&idx| ctx.queue.get(idx))
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    return Ok(PaneAction::QueueMoveUp(ids));
                }
                return Ok(PaneAction::Handled);
            }

            ListAction::MoveDown(indices) => {
                // Convert indices to queue IDs and move down
                let ids: Vec<u32> = indices
                    .iter()
                    .filter_map(|&idx| ctx.queue.get(idx))
                    .filter_map(|s| s.id)
                    .collect();
                if !ids.is_empty() {
                    ctx.render()?;
                    return Ok(PaneAction::QueueMoveDown(ids));
                }
                return Ok(PaneAction::Handled);
            }

            ListAction::Back => {
                // Clear marks first if any
                if self.list_view.has_marked() {
                    self.list_view.clear_marks();
                    ctx.render()?;
                    return Ok(PaneAction::Handled);
                }
                return Ok(PaneAction::BackPane);
            }

            ListAction::Passthrough => {
                // Handle queue-specific keys not covered by InteractiveListView
            }
        }

        // Handle Right arrow - navigate to artist
        if matches!(key.code(), KeyCode::Right) {
            if let Some(song) = self.selected_song(ctx) {
                if let Some(artist_id) = song.metadata.get("artist_browse_id").and_then(|v| v.first()) {
                    let artist_name = song.artist().unwrap_or("Artist").to_string();
                    key.stop_propagation();
                    return Ok(PaneAction::NavigateTo(EntityRef {
                        entity_type: DetailId::Artist,
                        id: artist_id.clone(),
                        name: artist_name,
                    }));
                }
            }
        }

        // Delegate remaining keys to legacy handle_action for full functionality
        // This preserves: QueueActions (DeleteAll, Shuffle, JumpToCurrent), etc.
        Pane::handle_action(self, key, ctx)?;

        Ok(PaneAction::Handled)
    }

    fn on_event(&mut self, event: &mut UiEvent, ctx: &Ctx) -> Result<()> {
        // Handle queue changes - validate selection
        if let UiEvent::Player = event {
            let len = ctx.queue.len();
            if let Some(idx) = self.list_view.selected() {
                if idx >= len {
                    self.list_view.select(if len > 0 { Some(len - 1) } else { None });
                }
            }
        }
        
        // Sync to current playing song
        if let Some((idx, _)) = ctx.find_current_song_in_queue() {
            self.list_view.sync_to(Some(idx));
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
