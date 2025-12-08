//! Display traits for list items.
//!
//! Provides a simple, decoupled interface for domain types to expose
//! their display data without depending on UI framework details.

use std::borrow::Cow;

use ratatui::style::Style;

/// Unified display trait for items rendered in lists.
///
/// Implementors provide data extraction; the widget handles layout and rendering.
/// This decouples domain models from the UI framework (ratatui).
///
/// # Example
/// ```ignore
/// impl ListItemDisplay for Song {
///     fn primary_text(&self) -> Cow<str> { self.title.as_str().into() }
///     fn secondary_text(&self) -> Option<Cow<str>> {
///         Some(format!("{} · {}", self.artist, self.album).into())
///     }
///     fn thumbnail_url(&self) -> Option<&str> { self.thumbnail.as_deref() }
///     fn type_icon(&self) -> &str { "🎵" }
/// }
/// ```
pub trait ListItemDisplay {
    /// Main title (song name, artist name, album title, etc.)
    fn primary_text(&self) -> Cow<'_, str>;

    /// Secondary line (artist · album · year)
    /// Returns None if no secondary text should be shown.
    fn secondary_text(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Thumbnail URL for rich list mode.
    /// Returns None if no thumbnail is available.
    fn thumbnail_url(&self) -> Option<&str> {
        None
    }

    /// Unicode icon representing the content type (🎵, 🎤, 💿, 📁)
    fn type_icon(&self) -> &str {
        ""
    }

    /// Style for the type icon (enables type-specific colors).
    /// Default: white. Override for semantic coloring per ui-ux-provised.md 4.1.
    fn icon_style(&self) -> Style {
        Style::default()
    }

    /// Duration or count text (e.g., "4:16" or "12 tracks")
    /// Displayed right-aligned, never truncated.
    fn duration_text(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Whether this item is currently playing.
    /// When true, renderer shows ▶ icon and accent color.
    fn is_playing(&self) -> bool {
        false
    }

    /// Whether this item is a section header (e.g., "Top Results", "Songs", "Artists").
    /// Headers are rendered with distinct styling: bold, centered, single row.
    fn is_header(&self) -> bool {
        false
    }
}
