//! Item list widget with support for compact and rich rendering modes.
//!
//! This widget renders a list of items that implement `ListItemDisplay`,
//! supporting both single-line compact mode and two-line rich mode with thumbnails.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, StatefulWidget, Widget},
};

use crate::ctx::Ctx;
use crate::domain::display::ListItemDisplay;
use super::element::Element;

/// Minimum terminal width for rich mode (columns)
const MIN_RICH_MODE_WIDTH: u16 = 60;

/// Rendering mode for the item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListRenderMode {
    /// Single-line, text-only (default, fast)
    #[default]
    Compact,
    /// Two-line with thumbnail and metadata
    Rich,
}

impl ListRenderMode {
    pub fn is_rich(&self) -> bool {
        matches!(self, Self::Rich)
    }
}

/// Configuration for the item list widget.
#[derive(Debug, Clone)]
pub struct ItemListConfig {
    /// Render mode (Compact or Rich)
    pub mode: ListRenderMode,
    /// Width of thumbnail in columns (for Rich mode)
    pub thumbnail_width: u16,
    /// Height of each row in lines (2 for Rich mode)
    pub row_height: u16,
}

impl Default for ItemListConfig {
    fn default() -> Self {
        Self {
            mode: ListRenderMode::Compact,
            thumbnail_width: 6,
            row_height: 3,
        }
    }
}

/// A list widget that renders items implementing `ListItemDisplay`.
///
/// Supports both compact (1-line) and rich (2-line + thumbnail) modes.
/// The mode is determined by configuration and terminal width.
pub struct ItemListWidget<'a, T> {
    items: &'a [T],
    config: ItemListConfig,
    highlight_style: Style,
    normal_style: Style,
    playing_style: Style,
    filter_match_style: Style,
    filter: Option<&'a str>,
    ctx: &'a Ctx,
}

impl<'a, T: ListItemDisplay> ItemListWidget<'a, T> {
    pub fn new(items: &'a [T], ctx: &'a Ctx) -> Self {
        use ratatui::style::Color;
        Self {
            items,
            config: ItemListConfig::default(),
            highlight_style: Style::default().add_modifier(Modifier::REVERSED),
            normal_style: Style::default(),
            playing_style: Style::default().add_modifier(Modifier::BOLD),
            filter_match_style: Style::default().fg(Color::Blue),
            filter: None,
            ctx,
        }
    }

    pub fn config(mut self, config: ItemListConfig) -> Self {
        self.config = config;
        self
    }

    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    pub fn normal_style(mut self, style: Style) -> Self {
        self.normal_style = style;
        self
    }

    pub fn playing_style(mut self, style: Style) -> Self {
        self.playing_style = style;
        self
    }

    /// Set the filter string for highlight matching
    pub fn filter(mut self, filter: Option<&'a str>) -> Self {
        self.filter = filter;
        self
    }

    /// Set the style for filter-matched items
    pub fn filter_match_style(mut self, style: Style) -> Self {
        self.filter_match_style = style;
        self
    }

    /// Determine effective render mode based on config and terminal width.
    /// Falls back to Compact when terminal is too narrow.
    fn effective_mode(&self, area: Rect) -> ListRenderMode {
        if area.width < MIN_RICH_MODE_WIDTH || !self.config.mode.is_rich() {
            ListRenderMode::Compact
        } else {
            self.config.mode
        }
    }

    /// Render in compact mode using ratatui's List widget.
    fn render_compact(&self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| {
                let icon = item.type_icon();
                let text = item.primary_text();
                let is_playing = item.is_playing();

                let prefix = if is_playing { "▶ " } else { "" };
                let line = format!("{}{} {}", prefix, icon, text);

                let style = if is_playing {
                    self.playing_style
                } else {
                    self.normal_style
                };

                ListItem::new(Line::styled(line, style))
            })
            .collect();

        let list = List::new(items).highlight_style(self.highlight_style);
        StatefulWidget::render(list, area, buf, state);
    }

    /// Render in rich mode with thumbnails and 2-line layout.
    fn render_rich(&self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
        use ratatui::style::Color;
        
        let row_height = self.config.row_height;
        
        // Calculate viewport capacity and ensure selected item is visible
        let selected = state.selected().unwrap_or(0);
        let mut offset = state.offset();
        
        // Calculate how many items fit in the viewport (approximate)
        // This is tricky because headers take 1 line, items take row_height lines
        let viewport_height = area.height as usize;
        
        // First, ensure offset doesn't make selected invisible
        // Calculate cumulative height from offset to selected
        let mut height_to_selected = 0usize;
        for i in offset..=selected.min(self.items.len().saturating_sub(1)) {
            if i >= self.items.len() { break; }
            let h = if self.items[i].is_header() { 1 } else { row_height as usize };
            height_to_selected += h;
        }
        
        // If selected is below viewport, scroll down
        while height_to_selected > viewport_height && offset < selected {
            let h = if self.items[offset].is_header() { 1 } else { row_height as usize };
            height_to_selected -= h;
            offset += 1;
        }
        
        // If selected is above offset, scroll up
        if selected < offset {
            offset = selected;
        }
        
        // Update state with new offset
        *state.offset_mut() = offset;
        
        let mut y = area.y;
        let mut item_idx = offset;

        // Render items, using different heights for headers vs normal items
        while y < area.y + area.height && item_idx < self.items.len() {
            let item = &self.items[item_idx];
            let is_header = item.is_header();
            let is_selected = state.selected() == Some(item_idx);
            let is_playing = item.is_playing();
            
            // Headers take 1 line, normal items take row_height lines
            let item_height = if is_header { 1 } else { row_height };
            
            // Don't render partial rows
            if y + item_height > area.y + area.height {
                break;
            }

            let row_rect = Rect {
                x: area.x,
                y,
                width: area.width,
                height: item_height,
            };

            if is_header {
                // HEADER: Bold, yellow, with separator line
                let header_style = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                
                // Render header text: "── Section Name ──────────"
                let text = item.primary_text();
                let separator = "───";
                let header_text = format!("{} {} ", separator, text);
                
                // Calculate remaining width in characters (not bytes!)
                let header_char_count = header_text.chars().count();
                let remaining_chars = (area.width as usize).saturating_sub(header_char_count);
                let full_line = format!("{}{}", header_text, "─".repeat(remaining_chars));
                
                // Truncate by characters, not bytes (safe for multi-byte chars)
                let display_str: String = full_line.chars().take(area.width as usize).collect();
                buf.set_string(row_rect.x, row_rect.y, &display_str, header_style);
            } else {
                // NORMAL ITEM: Check filter match
                let matches_filter = if let Some(filter) = self.filter {
                    !filter.is_empty() && item.filter_matches(filter)
                } else {
                    false
                };

                // Apply selection highlight to entire row
                if is_selected {
                    buf.set_style(row_rect, self.highlight_style);
                }

                // Build element tree for this row
                // Skip filter highlight if selected (selection highlight is enough, and combining them makes text unreadable)
                let show_filter_highlight = matches_filter && !is_selected;
                let elem = self.build_rich_row(item, is_playing, show_filter_highlight);
                elem.render(row_rect, buf, self.ctx);
            }

            y += item_height;
            item_idx += 1;
        }
    }

    /// Build the element tree for a rich list row.
    fn build_rich_row(&self, item: &'a T, is_playing: bool, matches_filter: bool) -> Element<'a> {
        let icon = item.type_icon();
        let prefix = if is_playing { "▶ " } else { "" };

        // Primary line: [prefix][icon] [title]
        let primary = format!("{}{} {}", prefix, icon, item.primary_text());

        // Secondary line: metadata (dimmed)
        let secondary = item
            .secondary_text()
            .unwrap_or_else(|| Cow::Borrowed(""));

        // Duration (right-aligned)
        let duration = item.duration_text().unwrap_or_else(|| Cow::Borrowed(""));

        // Primary text style: filter match takes priority, then playing, then normal
        let primary_style = if matches_filter {
            self.filter_match_style
        } else if is_playing {
            self.playing_style
        } else {
            self.normal_style
        };

        let secondary_style = if matches_filter {
            self.filter_match_style.add_modifier(Modifier::DIM)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };

        // Build the element tree following the layout:
        // [Thumbnail 4x2] [Gap] [Text Column] [Spacer] [Duration]
        Element::row_with_gap(
            vec![
                // Thumbnail
                Element::image(
                    item.thumbnail_url().map(String::from),
                    self.config.thumbnail_width,
                    self.config.row_height,
                ),
                // Text column (2 lines)
                Element::column(vec![
                    Element::styled_text(Cow::Owned(primary), primary_style),
                    Element::styled_text(secondary, secondary_style),
                ]),
                // Spacer to push duration right
                Element::spacer(),
                // Duration
                Element::styled_text(duration, self.normal_style),
            ],
            1, // gap
        )
    }
}

impl<'a, T: ListItemDisplay> StatefulWidget for ItemListWidget<'a, T> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        match self.effective_mode(area) {
            ListRenderMode::Compact => self.render_compact(area, buf, state),
            ListRenderMode::Rich => self.render_rich(area, buf, state),
        }
    }
}
