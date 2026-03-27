use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Widget},
};
use ratatui_image::Image;

use crate::shared::image_cache::{ImageCache, ThumbnailSize};

pub struct AsyncImage<'a> {
    cache: &'a ImageCache,
    url: Option<String>,
    size: ThumbnailSize,
    #[allow(dead_code)]
    placeholder_char: char,
}

impl<'a> AsyncImage<'a> {
    pub fn new(cache: &'a ImageCache, url: Option<String>) -> Self {
        Self { cache, url, size: ThumbnailSize::ListItem, placeholder_char: '⣿' }
    }

    /// Set the thumbnail size for this image
    pub fn size(mut self, size: ThumbnailSize) -> Self {
        self.size = size;
        self
    }

    pub fn placeholder_char(mut self, c: char) -> Self {
        self.placeholder_char = c;
        self
    }
}

impl<'a> Widget for AsyncImage<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(url) = self.url {
            // Use the proper size-aware API
            if let Some(protocol_arc) = self.cache.get_protocol(&url, self.size) {
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
