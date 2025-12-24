use std::borrow::Cow;

use ratatui::{
    text::{Line, Span},
    style::{Color, Style},
    widgets::{ListItem, ListState, TableState},
};

mod dir;
mod path;
mod stack;
mod state;
mod walk;
pub(crate) use dir::Dir;
pub use path::Path;
pub(crate) use stack::DirStack;
pub(crate) use state::DirState;
pub(crate) use walk::WalkDirStackItem;

use super::dir_or_song::DirOrSong;
use crate::{
    config::theme::properties::{Property, SongProperty},
    ctx::Ctx,
    domain::Song,
    backends::messaging::PreviewGroup,
    ui::panes::browser::SongExt,
};

pub(crate) trait DirStackItem {
    fn as_path(&self) -> &str;
    fn is_file(&self) -> bool;
    fn to_file_preview(&self, ctx: &Ctx) -> Vec<PreviewGroup>;
    fn matches(&self, song_format: &[Property<SongProperty>], ctx: &Ctx, filter: &str) -> bool;
    fn to_list_item<'a>(
        &self,
        ctx: &Ctx,
        is_marked: bool,
        matches_filter: bool,
        additional_content: Option<String>,
    ) -> ListItem<'a>;
    fn to_list_item_simple<'a>(&self, ctx: &Ctx) -> ListItem<'a> {
        self.to_list_item(ctx, false, false, None)
    }
    
    /// Whether this item can receive focus during navigation.
    /// Headers and other non-interactive items should return false.
    /// Navigation will skip non-focusable items.
    fn is_focusable(&self) -> bool {
        true
    }
}

impl DirStackItem for DirOrSong {
    fn as_path(&self) -> &str {
        match self {
            DirOrSong::Dir { name, .. } => name,
            DirOrSong::Song(s) => &s.uri,
        }
    }

    fn is_file(&self) -> bool {
        match self {
            DirOrSong::Dir { .. } => false,
            DirOrSong::Song(_) => true,
        }
    }

    fn to_file_preview(&self, ctx: &Ctx) -> Vec<PreviewGroup> {
        match self {
            DirOrSong::Dir { .. } => Vec::new(),
            DirOrSong::Song(s) => s.to_file_preview(ctx),
        }
    }

    fn matches(&self, song_format: &[Property<SongProperty>], ctx: &Ctx, filter: &str) -> bool {
        match self {
            DirOrSong::Dir { name, .. } => if name.is_empty() { "Untitled" } else { name.as_str() }
                .to_lowercase()
                .contains(&filter.to_lowercase()),
            DirOrSong::Song(s) => SongExt::matches(s, song_format, filter, ctx),
        }
    }

    fn to_list_item<'a>(
        &self,
        ctx: &Ctx,
        is_marked: bool,
        matches_filter: bool,
        additional_content: Option<String>,
    ) -> ListItem<'a> {
        match self {
            DirOrSong::Dir { name, playlist: is_playlist, .. } => {
                let config = &ctx.config;
                let marker_span = if is_marked {
                    Span::styled(
                        config.theme.symbols.marker.clone(),
                        config.theme.highlighted_item_style,
                    )
                } else {
                    Span::from(" ".repeat(config.theme.symbols.marker.chars().count()))
                };
                let mut value = Line::from(vec![
                    marker_span,
                    if *is_playlist {
                        Span::styled(
                            config.theme.symbols.playlist.clone(),
                            config.theme.symbols.playlist_style.unwrap_or_default(),
                        )
                    } else {
                        Span::styled(
                            config.theme.symbols.dir.clone(),
                            config.theme.symbols.dir_style.unwrap_or_default(),
                        )
                    },
                    Span::from(" "),
                    Span::from(if name.is_empty() {
                        Cow::Borrowed("Untitled")
                    } else {
                        Cow::Owned(name.to_owned())
                    }),
                ]);

                if let Some(content) = additional_content {
                    value.push_span(Span::raw(content));
                }
                if matches_filter {
                    ListItem::from(value).style(config.theme.highlighted_item_style)
                } else {
                    ListItem::from(value)
                }
            }
            DirOrSong::Song(s) => {
                s.to_list_item(ctx, is_marked, matches_filter, additional_content)
            }
        }
    }
}

impl DirStackItem for Song {
    fn as_path(&self) -> &str {
        &self.uri
    }

    fn is_file(&self) -> bool {
        true
    }

    fn to_file_preview(&self, ctx: &Ctx) -> Vec<PreviewGroup> {
        let key_style = ctx.config.theme.preview_label_style;
        let group_style = ctx.config.theme.preview_metadata_group_style;
        self.to_preview(key_style, group_style, ctx)
    }

    fn matches(&self, song_format: &[Property<SongProperty>], ctx: &Ctx, filter: &str) -> bool {
        // First try the song_format-based matching
        if SongExt::matches(self, song_format, filter, ctx) {
            return true;
        }
        // Fallback: also check title and artist directly (for consistency with filter_matches)
        let filter_lower = filter.to_lowercase();
        if let Some(title) = self.metadata.get("title").and_then(|v| v.first()) {
            if title.to_lowercase().contains(&filter_lower) {
                return true;
            }
        }
        if let Some(artist) = self.metadata.get("artist").and_then(|v| v.first()) {
            if artist.to_lowercase().contains(&filter_lower) {
                return true;
            }
        }
        if let Some(album) = self.metadata.get("album").and_then(|v| v.first()) {
            if album.to_lowercase().contains(&filter_lower) {
                return true;
            }
        }
        false
    }

    fn to_list_item<'a>(
        &self,
        ctx: &Ctx,
        is_marked: bool,
        matches_filter: bool,
        additional_content: Option<String>,
    ) -> ListItem<'a> {
        let config = &ctx.config;
        let item_type = self.metadata.get("type").and_then(|v| v.first()).map(|s| s.as_str()).unwrap_or("song");

        if item_type == "header" {
            let title = self.metadata.get("title").and_then(|v| v.first()).map(|s| s.as_str()).unwrap_or("Unknown");
            let style = Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::BOLD);
            return ListItem::new(Line::from(vec![
                Span::styled(format!("--- {} ---", title), style)
            ]));
        }

        let marker_span = if is_marked {
            Span::styled(config.theme.symbols.marker.clone(), config.theme.highlighted_item_style)
        } else {
            Span::from(" ".repeat(config.theme.symbols.marker.chars().count()))
        };

        let symbol = match item_type {
            "artist" => " ".to_string(), // Nerd Font icon for person/artist
            "album" => "jm ".to_string(), // Nerd Font icon for disc/album
            "video" => " ".to_string(), // Nerd Font icon for video
            _ => config.theme.symbols.song.clone(),
        };

        let mut spans = vec![
            marker_span,
            Span::styled(
                symbol,
                config.theme.symbols.song_style.unwrap_or_default(),
            ),
            Span::from(" "),
        ];

        if item_type == "artist" {
             if let Some(artist) = self.metadata.get("title").and_then(|v| v.first()) {
                spans.push(Span::styled(artist.to_string(), Style::default().add_modifier(ratatui::style::Modifier::BOLD)));
             }
        } else if item_type == "album" {
            if let Some(album) = self.metadata.get("title").and_then(|v| v.first()) {
                spans.push(Span::styled(album.to_string(), Style::default().add_modifier(ratatui::style::Modifier::BOLD)));
            }
            if let Some(artist) = self.metadata.get("artist").and_then(|v| v.first()) {
                spans.push(Span::from(format!(" by {}", artist)));
            }
            if let Some(year) = self.metadata.get("year").and_then(|v| v.first()) {
                spans.push(Span::from(format!(" ({})", year)));
            }
        } else if item_type == "playlist" {
            // Playlists: show title and author
            if let Some(title) = self.metadata.get("title").and_then(|v| v.first()) {
                spans.push(Span::styled(title.to_string(), Style::default().add_modifier(ratatui::style::Modifier::BOLD)));
            }
            if let Some(subtitle) = self.metadata.get("subtitle").and_then(|v| v.first()) {
                spans.push(Span::from(format!(" - {}", subtitle)));
            }
        } else if item_type == "video" {
            // Videos: show title and channel
            if let Some(title) = self.metadata.get("title").and_then(|v| v.first()) {
                spans.push(Span::styled(title.to_string(), Style::default()));
            }
            if let Some(artist) = self.metadata.get("artist").and_then(|v| v.first()) {
                spans.push(Span::styled(format!(" - {}", artist), Style::default().fg(Color::DarkGray)));
            }
        } else {
            // Default song rendering
            spans.extend(config.theme.browser_song_format.0.iter().map(|prop| {
                Span::from(
                    prop.as_string(
                        Some(self),
                        &config.theme.format_tag_separator,
                        config.theme.multiple_tag_resolution_strategy,
                        ctx,
                    )
                    .unwrap_or_default(),
                )
            }));
        }

        let mut value = Line::from(spans);

        if let Some(content) = additional_content {
            value.push_span(Span::raw(content));
        }
        if matches_filter {
            ListItem::from(value).style(config.theme.highlighted_item_style)
        } else {
            ListItem::from(value)
        }
    }

    fn is_focusable(&self) -> bool {
        // Headers should not receive focus during navigation
        self.item_type() != Some("header")
    }
}

pub trait ScrollingState {
    fn select_scrolling(&mut self, idx: Option<usize>);
    fn get_selected_scrolling(&self) -> Option<usize>;
    fn offset(&self) -> usize;
    fn set_offset(&mut self, value: usize);
}

impl ScrollingState for TableState {
    fn select_scrolling(&mut self, idx: Option<usize>) {
        self.select(idx);
    }

    fn get_selected_scrolling(&self) -> Option<usize> {
        self.selected()
    }

    fn offset(&self) -> usize {
        self.offset()
    }

    fn set_offset(&mut self, value: usize) {
        *self.offset_mut() = value;
    }
}

impl ScrollingState for ListState {
    fn select_scrolling(&mut self, idx: Option<usize>) {
        self.select(idx);
    }

    fn get_selected_scrolling(&self) -> Option<usize> {
        self.selected()
    }

    fn offset(&self) -> usize {
        self.offset()
    }

    fn set_offset(&mut self, value: usize) {
        *self.offset_mut() = value;
    }
}

#[cfg(test)]
impl DirStackItem for String {
    fn as_path(&self) -> &str {
        self
    }

    fn is_file(&self) -> bool {
        true
    }

    fn to_file_preview(&self, _ctx: &Ctx) -> Vec<PreviewGroup> {
        Vec::new()
    }

    fn matches(&self, _: &[Property<SongProperty>], _ctx: &Ctx, filter: &str) -> bool {
        self.to_lowercase().contains(&filter.to_lowercase())
    }

    fn to_list_item<'a>(
        &self,
        ctx: &Ctx,
        is_marked: bool,
        matches_filter: bool,
        _additional_content: Option<String>,
    ) -> ListItem<'a> {
        let config = &ctx.config;
        let marker_span = if is_marked {
            Span::styled(config.theme.symbols.marker.clone(), config.theme.highlighted_item_style)
        } else {
            Span::from(" ".repeat(config.theme.symbols.marker.chars().count()))
        };

        if matches_filter {
            ListItem::new(Line::from(vec![marker_span, Span::from(self.clone())]))
                .style(config.theme.highlighted_item_style)
        } else {
            ListItem::new(Line::from(vec![marker_span, Span::from(self.clone())]))
        }
    }
}
