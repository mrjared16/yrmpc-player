use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Widget},
};
use ratatui_image::Image;

use crate::shared::image_cache::ImageCache;

pub struct AsyncImage<'a> {
    cache: &'a ImageCache,
    url: Option<String>,
    placeholder_char: char,
}

impl<'a> AsyncImage<'a> {
    pub fn new(cache: &'a ImageCache, url: Option<String>) -> Self {
        Self {
            cache,
            url,
            placeholder_char: '⣿',
        }
    }

    pub fn placeholder_char(mut self, c: char) -> Self {
        self.placeholder_char = c;
        self
    }
}

impl<'a> Widget for AsyncImage<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(url) = self.url {
            if let Some(protocol_arc) = self.cache.get(&url) {
                if let Ok(protocol) = protocol_arc.lock() {
                    let image = Image::new(&*protocol);
                    image.render(area, buf);
                    return;
                }
            }
        }

        // Placeholder
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray))
            .render(area, buf);
    }
}
