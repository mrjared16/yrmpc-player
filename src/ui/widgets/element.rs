//! Internal element tree for flexible list item rendering.
//!
//! This module provides a compositional rendering primitive similar to React elements.
//! Elements can be composed into trees to describe complex layouts (row, column, image + text).
//!
//! **Note:** This is an internal implementation detail, not public API.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Text},
    widgets::{Paragraph, Widget},
};

use crate::ctx::Ctx;
use crate::ui::widgets::async_image::AsyncImage;

/// Width of an icon cell (icon character + space)
const ICON_CELL_WIDTH: u16 = 2;

/// Internal rendering primitive for composable UI elements.
///
/// Elements form a tree structure that the renderer walks to produce output.
/// This allows flexible layouts without hardcoding in the widget.
#[derive(Debug, Clone)]
pub(crate) enum Element<'a> {
    /// Plain styled text
    Text {
        content: Cow<'a, str>,
        style: Style,
    },

    /// Async-loaded image with placeholder
    Image {
        url: Option<String>,
        width: u16,
        height: u16,
    },

    /// Icon character with style (for type indicators)
    Icon { char: char, style: Style },

    /// Horizontal layout of children
    Row {
        children: Vec<Element<'a>>,
        gap: u16,
    },

    /// Vertical layout of children
    Column { children: Vec<Element<'a>> },

    /// Flexible spacer (takes remaining space)
    Spacer,
}

impl<'a> Element<'a> {
    /// Create a text element
    pub fn text(content: impl Into<Cow<'a, str>>) -> Self {
        Element::Text {
            content: content.into(),
            style: Style::default(),
        }
    }

    /// Create a styled text element
    pub fn styled_text(content: impl Into<Cow<'a, str>>, style: Style) -> Self {
        Element::Text {
            content: content.into(),
            style,
        }
    }

    /// Create an image element
    pub fn image(url: Option<String>, width: u16, height: u16) -> Self {
        Element::Image { url, width, height }
    }

    /// Create an icon element
    pub fn icon(char: char, style: Style) -> Self {
        Element::Icon { char, style }
    }

    /// Create a horizontal row of elements
    pub fn row(children: Vec<Element<'a>>) -> Self {
        Element::Row { children, gap: 1 }
    }

    /// Create a horizontal row with custom gap
    pub fn row_with_gap(children: Vec<Element<'a>>, gap: u16) -> Self {
        Element::Row { children, gap }
    }

    /// Create a vertical column of elements
    pub fn column(children: Vec<Element<'a>>) -> Self {
        Element::Column { children }
    }

    /// Create a spacer element
    pub fn spacer() -> Self {
        Element::Spacer
    }

    /// Render this element to the buffer
    pub fn render(&self, area: Rect, buf: &mut Buffer, ctx: &Ctx) {
        match self {
            Element::Text { content, style } => {
                let paragraph = Paragraph::new(content.as_ref()).style(*style);
                paragraph.render(area, buf);
            }

            Element::Image { url, .. } => {
                log::trace!("[DIAG-IMG] Element::render Image url={:?} area={:?}", url, area);
                let widget = AsyncImage::new(&ctx.image_cache, url.clone());
                widget.render(area, buf);
            }

            Element::Icon { char, style } => {
                if area.width > 0 && area.height > 0 {
                    buf.set_string(area.x, area.y, char.to_string(), *style);
                }
            }

            Element::Row { children, gap } => {
                self.render_row(children, *gap, area, buf, ctx);
            }

            Element::Column { children } => {
                self.render_column(children, area, buf, ctx);
            }

            Element::Spacer => {
                // Spacer renders nothing, just takes space
            }
        }
    }

    fn render_row(
        &self,
        children: &[Element<'a>],
        gap: u16,
        area: Rect,
        buf: &mut Buffer,
        ctx: &Ctx,
    ) {
        if children.is_empty() {
            return;
        }

        // Calculate constraints for each child
        let constraints: Vec<Constraint> = children
            .iter()
            .enumerate()
            .flat_map(|(i, child)| {
                let child_constraint = match child {
                    Element::Image { width, .. } => Constraint::Length(*width),
                    Element::Icon { .. } => Constraint::Length(ICON_CELL_WIDTH),
                    Element::Spacer => Constraint::Min(0),
                    Element::Text { content, .. } => {
                        // Text takes minimum needed or fills
                        let len = content.chars().count() as u16;
                        Constraint::Length(len.min(area.width))
                    }
                    Element::Row { .. } | Element::Column { .. } => Constraint::Min(0),
                };

                // Add gap after each child except the last
                if i < children.len() - 1 && gap > 0 {
                    vec![child_constraint, Constraint::Length(gap)]
                } else {
                    vec![child_constraint]
                }
            })
            .collect();

        let areas = Layout::horizontal(constraints).split(area);

        // Render children (skipping gap areas)
        let mut area_idx = 0;
        for child in children {
            if area_idx < areas.len() {
                child.render(areas[area_idx], buf, ctx);
            }
            area_idx += 2; // Skip the gap area
        }
    }

    fn render_column(&self, children: &[Element<'a>], area: Rect, buf: &mut Buffer, ctx: &Ctx) {
        if children.is_empty() {
            return;
        }

        let row_height = area.height / children.len() as u16;

        for (i, child) in children.iter().enumerate() {
            let child_area = Rect {
                x: area.x,
                y: area.y + (i as u16 * row_height),
                width: area.width,
                height: row_height.min(1), // Each line is 1 row
            };
            child.render(child_area, buf, ctx);
        }
    }
}
