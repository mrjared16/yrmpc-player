//! LibraryTabPane - TabPane for saved playlists and library content.
//!
//! Displays:
//! - Saved playlists
//! - Liked songs (future)
//! - Downloaded content (future)
//!
//! Implements TabPane trait for Navigator integration.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    prelude::Rect,
    widgets::{Block, Borders},
};

use crate::{
    config::keys::CommonAction,
    ctx::Ctx,
    domain::{
        DetailItem, Song,
        content::{ContentRef, ContentType},
    },
    shared::key_event::KeyEvent,
    ui::{
        panes::navigator_types::{
            BackspaceResult, DetailId, EntityRef, EscResult, InputMode, NavigatorPane, PaneAction,
            PaneId, TabId, TabPane,
        },
        widgets::selectable_list::{NavConfig, SelectableList},
    },
};

// =============================================================================
// LIBRARY TAB PANE
// =============================================================================

/// TabPane for library content (saved playlists, etc.)
#[derive(Debug, Clone, Default)]
pub struct LibraryTabPane {
    /// List view for navigation
    list_view: SelectableList,
    /// Cached playlist items
    playlists: Vec<ContentRef>,
}

impl LibraryTabPane {
    pub fn new(_ctx: &Ctx) -> Self {
        Self { list_view: SelectableList::new(), playlists: Vec::new() }
    }

    /// Set playlists to display
    pub fn set_playlists(&mut self, playlists: Vec<ContentRef>) {
        self.playlists = playlists;
        // Reset selection
        if !self.playlists.is_empty() {
            self.list_view.select(Some(0));
        } else {
            self.list_view.select(None);
        }
    }

    /// Get selected playlist
    fn selected_playlist(&self) -> Option<&ContentRef> {
        self.list_view.selected().and_then(|idx| self.playlists.get(idx))
    }

    fn is_char(key: &KeyEvent, ch: char) -> bool {
        matches!(key.code(), KeyCode::Char(c) if c == ch)
    }

    fn get_char(key: &KeyEvent) -> Option<char> {
        match key.code() {
            KeyCode::Char(c) => Some(c),
            _ => None,
        }
    }

    fn handle_navigation(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> bool {
        if let Some(action) = key.as_common_action(ctx) {
            match action {
                CommonAction::Down => {
                    self.list_view.select_next(&self.playlists, NavConfig::default());
                }
                CommonAction::Up => {
                    self.list_view.select_prev(&self.playlists, NavConfig::default());
                }
                CommonAction::Top => {
                    self.list_view.select_first(&self.playlists);
                }
                CommonAction::Bottom => {
                    self.list_view.select_last(&self.playlists);
                }
                CommonAction::DownHalf => {
                    self.list_view.next_half_viewport(&self.playlists, NavConfig::default());
                }
                CommonAction::UpHalf => {
                    self.list_view.prev_half_viewport(&self.playlists, NavConfig::default());
                }
                _ => return false,
            }
            return true;
        }
        false
    }
}

// =============================================================================
// NAVIGATOR PANE IMPLEMENTATION
// =============================================================================

impl NavigatorPane for LibraryTabPane {
    fn id(&self) -> PaneId {
        PaneId::Tab(TabId::Library)
    }

    fn mode(&self) -> InputMode {
        if self.list_view.is_find_mode() { InputMode::Find } else { InputMode::Normal }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        let config = &ctx.config;

        // Build title
        let title = format!(" Library ({}) ", self.playlists.len());

        // Add find indicator
        let title = if let Some(filter_display) = self.list_view.filter_display() {
            format!("{}{} ", title.trim_end(), filter_display)
        } else {
            title
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(config.as_border_style());

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Update viewport
        self.list_view.set_viewport_height(inner.height);

        // Render playlists
        self.list_view.render_simple(frame, inner, ctx, &self.playlists, None);

        Ok(())
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        let mode = self.mode();

        match mode {
            InputMode::Find => {
                if let Some(ch) = Self::get_char(key) {
                    if self.list_view.handle_find_char(&self.playlists, ch) {
                        key.stop_propagation();
                        return Ok(PaneAction::Handled);
                    }
                }

                if matches!(key.code(), KeyCode::Esc) {
                    self.list_view.handle_esc();
                    key.stop_propagation();
                    return Ok(PaneAction::Handled);
                }

                if matches!(key.code(), KeyCode::Enter) {
                    self.list_view.exit_find_mode(true);
                    key.stop_propagation();
                    return Ok(PaneAction::Handled);
                }

                if matches!(key.code(), KeyCode::Tab) {
                    self.list_view.confirm_find_and_jump_to_next();
                    key.stop_propagation();
                    return Ok(PaneAction::Handled);
                }

                if matches!(key.code(), KeyCode::Backspace) {
                    self.list_view.handle_backspace(&self.playlists);
                    key.stop_propagation();
                    return Ok(PaneAction::Handled);
                }
            }

            InputMode::Normal => {
                // Enter find mode with /
                if Self::is_char(key, '/') {
                    self.list_view.enter_find_mode(&self.playlists, "");
                    key.stop_propagation();
                    return Ok(PaneAction::Handled);
                }

                // Handle Esc
                if matches!(key.code(), KeyCode::Esc) {
                    match self.list_view.handle_esc() {
                        EscResult::Handled => {
                            key.stop_propagation();
                            return Ok(PaneAction::Handled);
                        }
                        EscResult::BackPane => {
                            key.stop_propagation();
                            return Ok(PaneAction::BackPane);
                        }
                    }
                }

                // Handle Backspace
                if matches!(key.code(), KeyCode::Backspace) {
                    match self.list_view.handle_backspace(&self.playlists) {
                        BackspaceResult::Handled => {
                            key.stop_propagation();
                            return Ok(PaneAction::Handled);
                        }
                        BackspaceResult::NoEffect => {
                            // Library has no stack
                        }
                    }
                    return Ok(PaneAction::Handled);
                }

                // Handle Enter - navigate to playlist
                if matches!(key.code(), KeyCode::Enter) {
                    if let Some(playlist) = self.selected_playlist() {
                        key.stop_propagation();
                        return Ok(PaneAction::NavigateTo(EntityRef {
                            entity_type: DetailId::Playlist,
                            id: playlist.id.clone(),
                            name: playlist.name.clone(),
                        }));
                    }
                    return Ok(PaneAction::Handled);
                }

                // Handle n/N for find navigation
                if self.list_view.is_filtering() {
                    if Self::is_char(key, 'n') {
                        self.list_view.filter_next_match();
                        key.stop_propagation();
                        return Ok(PaneAction::Handled);
                    }
                    if Self::is_char(key, 'N') {
                        self.list_view.filter_prev_match();
                        key.stop_propagation();
                        return Ok(PaneAction::Handled);
                    }
                }

                // Handle navigation
                if self.handle_navigation(key, ctx) {
                    return Ok(PaneAction::Handled);
                }
            }

            InputMode::Edit => {
                // Library doesn't use Edit mode
            }
        }

        Ok(PaneAction::Handled)
    }
}

// =============================================================================
// TAB PANE IMPLEMENTATION
// =============================================================================

impl TabPane for LibraryTabPane {
    fn tab_id(&self) -> TabId {
        TabId::Library
    }

    fn current_stage(&self) -> &str {
        "List"
    }

    fn can_go_back_stage(&self) -> bool {
        false
    }

    fn go_back_stage(&mut self) -> bool {
        false
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_pane_id() {
        let pane = LibraryTabPane::default();
        assert_eq!(pane.id(), PaneId::Tab(TabId::Library));
        assert_eq!(pane.tab_id(), TabId::Library);
    }

    #[test]
    fn test_library_pane_mode() {
        let pane = LibraryTabPane::default();
        assert_eq!(pane.mode(), InputMode::Normal);
    }
}
