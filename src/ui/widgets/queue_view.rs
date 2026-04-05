//! Unified QueueView component for both Modal and Tab contexts.
//!
//! Provides both rendering (compact/full modes) and queue operations
//! (play, delete, navigate) to avoid code duplication.

use std::borrow::Cow;

use anyhow::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, ListState, Paragraph},
};

use crate::{
    ctx::Ctx,
    domain::{
        Song,
        display::{ListItemDisplay, SearchKey},
    },
    shared::image_cache::ThumbnailSize,
    ui::widgets::{
        async_image::AsyncImage,
        item_list::{ItemListConfig, ItemListWidget, ListRenderMode},
    },
};

/// Wrapper that provides ListItemDisplay with playing context
struct QueueSongView<'a> {
    song: &'a Song,
    is_current: bool,
}

impl<'a> QueueSongView<'a> {
    fn new(song: &'a Song, is_current: bool) -> Self {
        Self { song, is_current }
    }
}

impl ListItemDisplay for QueueSongView<'_> {
    fn primary_text(&self) -> Cow<'_, str> {
        self.song.primary_text()
    }

    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        self.song.secondary_text()
    }

    fn thumbnail_url(&self) -> Option<&str> {
        self.song.thumbnail_url()
    }

    fn type_icon(&self) -> &str {
        self.song.type_icon()
    }

    fn duration_text(&self) -> Option<Cow<'_, str>> {
        self.song.duration_text()
    }

    fn is_playing(&self) -> bool {
        self.is_current
    }

    fn search_key(&self) -> SearchKey {
        self.song.search_key()
    }

    fn matches_folded_query(&self, folded_query: &str) -> bool {
        self.song.matches_folded_query(folded_query)
    }
}

/// Unified queue view component with rendering AND operations
#[derive(Debug, Default)]
pub struct QueueView {
    pub list_state: ListState,
}

impl QueueView {
    pub fn new() -> Self {
        Self::default()
    }

    // ========== NAVIGATION (Single Responsibility: Selection management)
    // ==========

    /// Move selection up
    pub fn select_previous(&mut self, queue_len: usize) {
        if queue_len == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Move selection down
    pub fn select_next(&mut self, queue_len: usize) {
        if queue_len == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => (i + 1).min(queue_len - 1),
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Get currently selected index
    pub fn selected(&self) -> Option<usize> {
        self.list_state.selected()
    }

    /// Select a specific index
    pub fn select(&mut self, index: Option<usize>) {
        self.list_state.select(index);
    }

    /// Sync selection to current playing song if not already set
    pub fn sync_to_current(&mut self, ctx: &Ctx) {
        if self.list_state.selected().is_none() {
            if let Some((idx, _)) = ctx.find_current_song_in_queue() {
                self.list_state.select(Some(idx));
            }
        }
    }

    // ========== OPERATIONS (Using QueueItemOps trait) ==========

    /// Play the selected song. Returns true if command was sent.
    pub fn play_selected(&self, ctx: &mut Ctx) -> bool {
        use crate::domain::{QueueItemAction, QueueItemOps};

        if let Some(idx) = self.selected() {
            // Clone song to release borrow on queue_state before calling execute
            let song = ctx.queue_state().get(idx);
            if let Some(song) = song {
                // Use the trait method - Tell, Don't Ask!
                if song.execute_queue_action(QueueItemAction::PlayOrToggle, ctx).is_ok() {
                    return true;
                }
            }
        }
        false
    }

    /// Delete the selected song from queue. Returns true if command was sent.
    pub fn delete_selected(&mut self, ctx: &mut Ctx) -> bool {
        use crate::domain::{QueueItemAction, QueueItemOps};

        if let Some(idx) = self.selected() {
            // Clone song to release borrow on queue_state before calling execute
            let song = ctx.queue_state().get(idx);
            if let Some(song) = song {
                // Use the trait method - Tell, Don't Ask!
                if song.execute_queue_action(QueueItemAction::Delete, ctx).is_ok() {
                    // Adjust selection after delete
                    let queue_len = ctx.queue_state().len();
                    if idx > 0 && queue_len > 1 {
                        self.select_previous(queue_len);
                    }
                    return true;
                }
            }
        }
        false
    }

    // ========== RENDERING (Single Responsibility: Display) ==========

    /// Render the queue view, adapting layout to available width
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        // Sync selection on first render
        self.sync_to_current(ctx);

        if area.width < 50 {
            self.render_compact(frame, area, ctx);
        } else {
            self.render_full(frame, area, ctx);
        }
    }

    /// Compact mode: list only (for sidebar modal)
    fn render_compact(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;

        // Find current song for playing indicator
        let current_song_id = ctx.find_current_song_in_queue().map(|(_, song)| song.id);

        // Get queue snapshot for rendering
        let queue = ctx.queue_state().read();

        // Create items with playing context
        let items: Vec<QueueSongView<'_>> = queue
            .iter()
            .map(|song| {
                let is_current = current_song_id.is_some_and(|id| id == song.id);
                QueueSongView::new(song, is_current)
            })
            .collect();

        // Configure compact rich list
        let item_config =
            ItemListConfig { mode: ListRenderMode::Rich, thumbnail_width: 4, row_height: 2 };

        let widget = ItemListWidget::new(&items, ctx)
            .config(item_config)
            .highlight_style(config.theme.current_item_style);

        // Render with header
        let queue_count = queue.len();
        let block = Block::default()
            .title(format!(" Queue ({}) ", queue_count))
            .borders(Borders::ALL)
            .border_style(config.as_border_style());

        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_stateful_widget(widget, inner, &mut self.list_state);
    }

    /// Full mode: album art + list (for queue tab)
    fn render_full(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        // Split into album art area and queue list
        let [art_area, list_area] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas::<2>(area);

        // Render album art (left side) - show now playing
        self.render_now_playing(frame, art_area, ctx);

        // Render queue list (right side)
        self.render_compact(frame, list_area, ctx);
    }

    /// Render the now playing section with album art
    fn render_now_playing(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;

        let block = Block::default()
            .title(" Now Playing ")
            .borders(Borders::ALL)
            .border_style(config.as_border_style());

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Get current song
        if let Some((_, song)) = ctx.find_current_song_in_queue() {
            // Split for album art and info
            let [img_area, info_area] =
                Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)])
                    .areas::<2>(inner);

            // Render album art if available
            let thumbnail_url = song.thumbnail_url().map(|s| s.to_string());
            let image =
                AsyncImage::new(&ctx.image_cache, thumbnail_url).size(ThumbnailSize::AlbumArt);
            frame.render_widget(image, img_area);

            // Render song info
            let title = song.title();
            let artist = song.artist().unwrap_or("Unknown Artist");

            let info = Paragraph::new(vec![
                Line::from(Span::styled(title, config.theme.current_item_style)),
                Line::from(Span::styled(artist, config.as_text_style())),
            ]);

            frame.render_widget(info, info_area);
        }
    }
}
