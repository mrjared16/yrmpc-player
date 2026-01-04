//! Queue side panel widget for overlay rendering.
//!
//! This widget renders a compact queue list that appears on the right side
//! of the screen when toggled with 'Q'.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, ListState, StatefulWidget, Widget},
};

use super::item_list::{ItemListConfig, ItemListWidget, ListRenderMode};
use crate::{
    ctx::Ctx,
    domain::{Song, display::ListItemDisplay},
};

/// Wrapper that provides ListItemDisplay with playing context for panel
struct PanelSongView<'a> {
    song: &'a Song,
    is_current: bool,
}

impl<'a> PanelSongView<'a> {
    fn new(song: &'a Song, is_current: bool) -> Self {
        Self { song, is_current }
    }
}

impl ListItemDisplay for PanelSongView<'_> {
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
}

/// Queue side panel widget
pub struct QueuePanel<'a> {
    ctx: &'a Ctx,
    list_state: ListState,
}

impl<'a> QueuePanel<'a> {
    pub fn new(ctx: &'a Ctx) -> Self {
        Self { ctx, list_state: ListState::default() }
    }

    /// Render the queue panel to the given area
    pub fn render(mut self, frame: &mut ratatui::Frame, area: Rect) {
        let config = &self.ctx.config;

        // Find current song ID for is_playing detection
        let current_song_id = self.ctx.find_current_song_in_queue().map(|(_, song)| song.id);

        // Get queue snapshot for rendering
        let queue = self.ctx.queue_store().read();

        // Create wrapper items with playing context
        let items: Vec<PanelSongView<'_>> = queue
            .iter()
            .map(|song| {
                let is_current = current_song_id.is_some_and(|id| id == song.id);
                PanelSongView::new(song, is_current)
            })
            .collect();

        // Select current song in list
        if let Some((idx, _)) = self.ctx.find_current_song_in_queue() {
            self.list_state.select(Some(idx));
        }

        // Configure for compact rich mode
        let item_config = ItemListConfig {
            mode: ListRenderMode::Rich,
            thumbnail_width: 4, // Smaller for panel
            row_height: 2,      // Compact rows
        };

        let widget = ItemListWidget::new(&items, self.ctx)
            .config(item_config)
            .highlight_style(config.theme.current_item_style);

        // Render with border
        let border_style = config.as_border_style();
        let block =
            Block::default().title(" Queue ").borders(Borders::ALL).border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_stateful_widget(widget, inner, &mut self.list_state);
    }
}
