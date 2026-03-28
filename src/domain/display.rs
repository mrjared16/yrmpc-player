//! Display traits for list items.
//!
//! Provides a simple, decoupled interface for domain types to expose
//! their display data without depending on UI framework details.

use std::borrow::Cow;

use ratatui::style::Style;

use crate::shared::string_util::fold_for_match;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchKey {
    primary: String,
    secondary: Option<String>,
}

impl SearchKey {
    #[must_use]
    pub fn new(primary: impl Into<String>, secondary: Option<String>) -> Self {
        Self { primary: primary.into(), secondary }
    }

    #[must_use]
    pub fn from_display(primary: &str, secondary: Option<&str>) -> Self {
        Self::new(fold_for_match(primary), secondary.map(fold_for_match))
    }

    #[must_use]
    pub fn matches(&self, folded_query: &str) -> bool {
        !folded_query.is_empty()
            && (self.primary.contains(folded_query)
                || self
                    .secondary
                    .as_ref()
                    .is_some_and(|secondary| secondary.contains(folded_query)))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primary.is_empty() && self.secondary.as_ref().is_none_or(String::is_empty)
    }
}

/// Unified display trait for items rendered in lists.
///
/// Implementors provide data extraction; the widget handles layout and
/// rendering. This decouples domain models from the UI framework (ratatui).
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
    /// Default: white. Override for semantic coloring per ui-ux-provised.md
    /// 4.1.
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

    /// Whether this item is next in playback order (for shuffle mode
    /// indicator). When true, renderer shows ▷ icon or "NEXT" label.
    fn is_next(&self) -> bool {
        false
    }

    /// Whether this item is a section header (e.g., "Top Results", "Songs",
    /// "Artists"). Headers are rendered with distinct styling: bold,
    /// centered, single row.
    fn is_header(&self) -> bool {
        false
    }

    /// Whether this item can receive focus/selection during navigation.
    /// Default: true for regular items, false for headers.
    /// Returning false causes navigation to skip this item.
    /// Future-proof: can be overridden to enable selectable headers for
    /// actions.
    fn is_focusable(&self) -> bool {
        !self.is_header()
    }

    /// Searchable text folded for fast matching.
    ///
    /// Override this to return cached/precomputed searchable text for stable
    /// item types.
    fn search_key(&self) -> SearchKey {
        SearchKey::from_display(
            self.primary_text().as_ref(),
            self.secondary_text().as_deref().map(str::trim),
        )
    }

    /// Check if this item matches a folded query string.
    fn matches_folded_query(&self, folded_query: &str) -> bool {
        self.search_key().matches(folded_query)
    }
}
