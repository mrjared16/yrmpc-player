//! Navigator - Central controller for pane navigation.
//!
//! The Navigator manages:
//! - Tab panes (Search, Queue, Library) - always available via hotkeys 1/2/3
//! - Detail panes (Artist, Album, Playlist) - shown when content exists
//! - Pane history for Esc-based navigation
//! - Routing key events to the active pane
//!
//! ## Navigation Flow
//!
//! 1. User presses Enter on an artist in search results
//! 2. SearchPaneV2::handle_key returns PaneAction::NavigateTo(EntityRef { Artist, id, name })
//! 3. Navigator receives the action, fetches artist content, pushes to ArtistDetailPane
//! 4. Navigator switches active pane to ArtistDetailPane, adds SearchPane to history
//! 5. User presses Esc → Navigator pops history, returns to SearchPane
//!
//! ## Coexistence with Legacy
//!
//! The Navigator can coexist with the legacy PaneContainer system.
//! During migration, both systems operate - Navigator manages its panes,
//! legacy system manages others. Full migration replaces PaneContainer.

use std::collections::HashMap;

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    ctx::Ctx,
    shared::key_event::KeyEvent,
};

use super::UiEvent;

use super::navigator_types::{
    DetailId, DetailPane, EntityContent, EntityRef, InputMode,
    MoveDirection, NavigatorPane, PaneAction, PaneId, TabId, TabPane,
};
use super::artist_detail::ArtistDetailPane;
use super::album_detail::AlbumDetailPane;
use super::playlist_detail::PlaylistDetailPane;
use super::library_tab::LibraryTabPane;
use super::queue_pane_v2::QueuePaneV2;
use super::search_pane_v2::SearchPaneV2;

// =============================================================================
// NAVIGATOR
// =============================================================================

/// Central navigator managing pane lifecycle and navigation.
pub struct Navigator {
    // Tab panes (always available)
    search_pane: SearchPaneV2,
    queue_pane: QueuePaneV2,
    library_pane: LibraryTabPane,

    // Detail panes (content stacking)
    artist_pane: ArtistDetailPane,
    album_pane: AlbumDetailPane,
    playlist_pane: PlaylistDetailPane,

    /// Currently active pane
    active: PaneId,

    /// Navigation history (for Esc to go back)
    history: Vec<PaneId>,

    /// Maximum history depth
    max_history: usize,
}

impl std::fmt::Debug for Navigator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Navigator")
            .field("active", &self.active)
            .field("history", &self.history)
            .field("max_history", &self.max_history)
            .finish_non_exhaustive()
    }
}

impl Navigator {
    /// Create a new Navigator with initialized panes.
    pub fn new(ctx: &Ctx) -> Self {
        Self {
            search_pane: SearchPaneV2::new(ctx),
            queue_pane: QueuePaneV2::new(ctx),
            library_pane: LibraryTabPane::new(ctx),
            artist_pane: ArtistDetailPane::new(),
            album_pane: AlbumDetailPane::new(),
            playlist_pane: PlaylistDetailPane::new(),
            active: PaneId::Tab(TabId::Search),
            history: Vec::new(),
            max_history: 10,
        }
    }

    // =========================================================================
    // PANE ACCESS
    // =========================================================================

    /// Get reference to the currently active pane.
    fn active_pane(&self) -> &dyn NavigatorPane {
        match self.active {
            PaneId::Tab(TabId::Search) => &self.search_pane,
            PaneId::Tab(TabId::Queue) => &self.queue_pane,
            PaneId::Tab(TabId::Library) => &self.library_pane,
            PaneId::Detail(DetailId::Artist) => &self.artist_pane,
            PaneId::Detail(DetailId::Album) => &self.album_pane,
            PaneId::Detail(DetailId::Playlist) => &self.playlist_pane,
        }
    }

    /// Get mutable reference to the currently active pane.
    fn active_pane_mut(&mut self) -> &mut dyn NavigatorPane {
        match self.active {
            PaneId::Tab(TabId::Search) => &mut self.search_pane,
            PaneId::Tab(TabId::Queue) => &mut self.queue_pane,
            PaneId::Tab(TabId::Library) => &mut self.library_pane,
            PaneId::Detail(DetailId::Artist) => &mut self.artist_pane,
            PaneId::Detail(DetailId::Album) => &mut self.album_pane,
            PaneId::Detail(DetailId::Playlist) => &mut self.playlist_pane,
        }
    }

    /// Get mutable reference to a detail pane.
    fn detail_pane_mut(&mut self, id: DetailId) -> &mut dyn DetailPane {
        match id {
            DetailId::Artist => &mut self.artist_pane,
            DetailId::Album => &mut self.album_pane,
            DetailId::Playlist => &mut self.playlist_pane,
        }
    }

    // =========================================================================
    // NAVIGATION
    // =========================================================================

    /// Switch to a specific pane, adding current to history.
    pub fn switch_to(&mut self, pane: PaneId) {
        if self.active == pane {
            return;
        }

        // Don't switch to empty detail panes
        if let PaneId::Detail(detail_id) = pane {
            let has_content = match detail_id {
                DetailId::Artist => self.artist_pane.has_content(),
                DetailId::Album => self.album_pane.has_content(),
                DetailId::Playlist => self.playlist_pane.has_content(),
            };
            if !has_content {
                return;
            }
        }

        // Add current to history
        self.history.push(self.active);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }

        self.active = pane;
    }

    /// Switch to a tab by hotkey (1/2/3).
    pub fn switch_to_tab(&mut self, tab: TabId) {
        self.switch_to(PaneId::Tab(tab));
    }

    /// Go back to previous pane in history.
    pub fn go_back(&mut self) -> bool {
        // Skip empty detail panes in history
        while let Some(prev) = self.history.pop() {
            if let PaneId::Detail(detail_id) = prev {
                let has_content = match detail_id {
                    DetailId::Artist => self.artist_pane.has_content(),
                    DetailId::Album => self.album_pane.has_content(),
                    DetailId::Playlist => self.playlist_pane.has_content(),
                };
                if !has_content {
                    continue; // Skip empty detail pane
                }
            }
            self.active = prev;
            return true;
        }
        false
    }

    /// Navigate to an entity (artist, album, playlist).
    ///
    /// This is called when a pane returns `PaneAction::NavigateTo`.
    /// The content should be fetched by the source pane before calling this.
    /// 
    /// # Navigation Flow
    /// 1. Source pane (e.g., SearchPaneV2) fetches entity content asynchronously
    /// 2. Source pane receives content via on_query_finished
    /// 3. Source pane calls Navigator::push_content() with the fetched content
    /// 4. Source pane returns PaneAction::NavigateTo
    /// 5. Navigator switches to the detail pane (content already pushed)
    ///
    /// This approach keeps async handling in panes that implement on_query_finished,
    /// while Navigator remains a synchronous controller.
    pub fn navigate_to(&mut self, entity: EntityRef, _ctx: &Ctx) {
        log::info!(
            "Navigator::navigate_to: {:?} id={} name={}",
            entity.entity_type,
            entity.id,
            entity.name
        );

        // Switch to the detail pane
        let target = PaneId::Detail(entity.entity_type);
        
        // Add current to history before switching
        if self.active != target {
            self.history.push(self.active);
            if self.history.len() > self.max_history {
                self.history.remove(0);
            }
        }

        self.active = target;

        // Content should already be pushed via push_content() before this is called.
        // If the detail pane is empty, log a warning.
        let has_content = match entity.entity_type {
            DetailId::Artist => self.artist_pane.has_content(),
            DetailId::Album => self.album_pane.has_content(),
            DetailId::Playlist => self.playlist_pane.has_content(),
        };
        
        if !has_content {
            log::warn!(
                "Navigator::navigate_to: {:?} pane has no content. \
                 Caller should push_content() before NavigateTo action.",
                entity.entity_type
            );
        }
    }

    /// Push content to a detail pane.
    pub fn push_content(&mut self, content: EntityContent) {
        let detail_id = content.detail_id();
        let pane = self.detail_pane_mut(detail_id);
        pane.push(content);

        // Switch to the pane if not already active
        let target = PaneId::Detail(detail_id);
        if self.active != target {
            self.history.push(self.active);
            self.active = target;
        }
    }

    // =========================================================================
    // KEY HANDLING
    // =========================================================================

    /// Handle a key event, routing to active pane and processing actions.
    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        use crossterm::event::KeyCode;

        // Get current mode - if in Edit or Find mode, block global hotkeys
        let current_mode = self.mode();
        let block_hotkeys = matches!(current_mode, InputMode::Edit | InputMode::Find);

        // Handle global hotkeys (1/2/3 for tabs) - ONLY in Normal mode
        if !block_hotkeys {
            match key.code() {
                KeyCode::Char('1') => {
                    self.switch_to_tab(TabId::Search);
                    key.stop_propagation();
                    ctx.render()?;
                    return Ok(());
                }
                KeyCode::Char('2') => {
                    self.switch_to_tab(TabId::Queue);
                    key.stop_propagation();
                    ctx.render()?;
                    return Ok(());
                }
                KeyCode::Char('3') => {
                    self.switch_to_tab(TabId::Library);
                    key.stop_propagation();
                    ctx.render()?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Route to active pane
        let action = self.active_pane_mut().handle_key(key, ctx)?;

        // Process returned action
        match action {
            PaneAction::Handled => {}
            PaneAction::BackPane => {
                if !self.go_back() {
                    // No history - stay on current pane
                    // Could default to a home pane here
                }
                ctx.render()?;
            }
            PaneAction::NavigateTo(entity) => {
                self.navigate_to(entity, ctx);
                ctx.render()?;
            }
            PaneAction::Play(song) => {
                // Execute play action via context
                self.execute_play(ctx, vec![song], 0)?;
            }
            PaneAction::PlayAll { songs, start_index } => {
                self.execute_play(ctx, songs, start_index)?;
            }
            PaneAction::Enqueue(songs) => {
                self.execute_enqueue(ctx, songs)?;
            }
            PaneAction::QueueDelete(ids) => {
                self.execute_queue_delete(ctx, ids)?;
            }
            #[allow(deprecated)]
            PaneAction::QueueMoveUp(ids) => {
                self.execute_queue_move(ctx, ids, MoveDirection::Up)?;
            }
            #[allow(deprecated)]
            PaneAction::QueueMoveDown(ids) => {
                self.execute_queue_move(ctx, ids, MoveDirection::Down)?;
            }
            PaneAction::QueueMove { ids, direction } => {
                self.execute_queue_move(ctx, ids, direction)?;
            }
            PaneAction::ShowModal(_kind) => {
                // TODO: Implement modal display
                log::info!("Navigator: ShowModal requested");
            }
            PaneAction::Search(query) => {
                // TODO: Implement search action
                log::info!("Navigator: Search requested: {}", query);
            }
        }

        Ok(())
    }

    /// Execute play action - replace queue and start playback.
    fn execute_play(&mut self, ctx: &mut Ctx, songs: Vec<crate::domain::Song>, start_index: usize) -> Result<()> {
        log::info!("Navigator: Playing {} songs starting at index {}", songs.len(), start_index);
        
        // Use ctx.command to execute synchronously on the backend
        ctx.command(move |client| {
            // Clear existing queue
            #[allow(deprecated)]
            client.clear()?;
            
            // Add all songs
            for song in &songs {
                #[allow(deprecated)]
                client.add_song(song, None)?;
            }
            
            // Start playback at start_index
            if !songs.is_empty() {
                #[allow(deprecated)]
                client.play_pos(start_index)?;
            }
            
            Ok(())
        });
        
        ctx.render()?;
        Ok(())
    }

    /// Execute enqueue action - add songs to queue without clearing.
    fn execute_enqueue(&mut self, ctx: &mut Ctx, songs: Vec<crate::domain::Song>) -> Result<()> {
        log::info!("Navigator: Enqueueing {} songs", songs.len());
        
        ctx.command(move |client| {
            for song in &songs {
                #[allow(deprecated)]
                client.add_song(song, None)?;
            }
            Ok(())
        });
        
        ctx.render()?;
        Ok(())
    }

    /// Execute queue delete action.
    fn execute_queue_delete(&mut self, ctx: &mut Ctx, ids: Vec<u32>) -> Result<()> {
        log::info!("Navigator: Deleting {} queue items", ids.len());
        
        ctx.command(move |client| {
            for id in &ids {
                #[allow(deprecated)]
                client.delete_id(*id)?;
            }
            Ok(())
        });
        
        ctx.render()?;
        Ok(())
    }

    /// Execute queue move action with unified direction.
    ///
    /// Optimized for the common case where selected items are neighbors (contiguous block).
    /// Instead of moving each item individually, we move the block as a unit by:
    /// - Moving up: move the item above block to after block
    /// - Moving down: move the item below block to before block
    fn execute_queue_move(&mut self, ctx: &mut Ctx, ids: Vec<u32>, direction: MoveDirection) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        log::info!("Navigator: Moving {} queue items {:?}", ids.len(), direction);

        let queue = &ctx.queue;
        let queue_len = queue.len();

        // Find positions of all selected items
        let mut positions: Vec<usize> = ids
            .iter()
            .filter_map(|&id| {
                queue.iter()
                    .position(|song| song.id == Some(id))
            })
            .collect();

        if positions.is_empty() {
            log::warn!("Navigator: No valid positions found for move");
            return Ok(());
        }

        // Sort to find the block boundaries
        positions.sort();

        let first_pos = positions[0];
        let last_pos = positions[positions.len() - 1];

        // Check if at boundary
        if direction == MoveDirection::Up && first_pos == 0 {
            log::debug!("Navigator: Already at top, cannot move up");
            return Ok(());
        }
        if direction == MoveDirection::Down && last_pos >= queue_len.saturating_sub(1) {
            log::debug!("Navigator: Already at bottom, cannot move down");
            return Ok(());
        }

        // Get the ID and target position for the single move command
        // For neighbors, moving the block requires just one operation:
        // - Move up: move item above the block to after the block
        // - Move down: move item below the block to before the block
        let (move_id, target_pos) = if direction == MoveDirection::Up {
            // Moving up: take the item ABOVE the block and move it BELOW the block
            let above_pos = first_pos - 1;
            let above_id = queue.get(above_pos).and_then(|s| s.id);
            if let Some(id) = above_id {
                (id, last_pos as u32)  // Move to last position of block
            } else {
                return Ok(());
            }
        } else {
            // Moving down: take the item BELOW the block and move it ABOVE the block
            let below_pos = last_pos + 1;
            let below_id = queue.get(below_pos).and_then(|s| s.id);
            if let Some(id) = below_id {
                (id, first_pos as u32)  // Move to first position of block
            } else {
                return Ok(());
            }
        };

        log::debug!("Navigator: Block move - moving id {} to position {}", move_id, target_pos);

        // Single command for the entire block move
        ctx.command(move |client| {
            #[allow(deprecated)]
            client.move_id(move_id, target_pos)?;
            Ok(())
        });

        ctx.render()?;
        Ok(())
    }

    // =========================================================================
    // RENDERING
    // =========================================================================

    /// Render the currently active pane.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        self.active_pane_mut().render(frame, area, ctx)
    }

    // =========================================================================
    // STATE QUERIES
    // =========================================================================

    /// Get the currently active pane ID.
    pub fn active_pane_id(&self) -> PaneId {
        self.active
    }

    /// Get the current input mode of the active pane.
    pub fn mode(&self) -> InputMode {
        self.active_pane().mode()
    }

    /// Get breadcrumb for display.
    pub fn breadcrumb(&self) -> String {
        match self.active {
            PaneId::Tab(tab) => tab.label().to_string(),
            PaneId::Detail(DetailId::Artist) => {
                if let Some(title) = self.artist_pane.current_title() {
                    format!("Artist > {}", title)
                } else {
                    "Artist".to_string()
                }
            }
            PaneId::Detail(DetailId::Album) => "Album".to_string(),
            PaneId::Detail(DetailId::Playlist) => "Playlist".to_string(),
        }
    }

    /// Get mutable reference to search pane (for external access).
    pub fn search_pane_mut(&mut self) -> &mut SearchPaneV2 {
        &mut self.search_pane
    }

    /// Get mutable reference to queue pane (for external access).
    pub fn queue_pane_mut(&mut self) -> &mut QueuePaneV2 {
        &mut self.queue_pane
    }

    // =========================================================================
    // EVENT HANDLING
    // =========================================================================

    /// Handle UI events (queue changes, playback state, etc.).
    /// Routes events to all panes that might care.
    pub(crate) fn on_event(&mut self, event: &mut UiEvent, ctx: &Ctx) -> Result<()> {
        // Route to all panes - they have default implementations that ignore irrelevant events
        self.search_pane.on_event(event, ctx)?;
        self.queue_pane.on_event(event, ctx)?;
        self.library_pane.on_event(event, ctx)?;
        self.artist_pane.on_event(event, ctx)?;
        self.album_pane.on_event(event, ctx)?;
        self.playlist_pane.on_event(event, ctx)?;
        Ok(())
    }

    /// Handle async query results.
    /// Routes to the active pane.
    pub(crate) fn on_query_finished(
        &mut self, 
        id: &'static str, 
        data: crate::QueryResult, 
        ctx: &Ctx,
    ) -> Result<()> {
        // Route to the active pane
        self.active_pane_mut().on_query_finished(id, data, ctx)
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full tests require Ctx which is complex to mock
    // These are basic structural tests

    #[test]
    fn test_pane_id_equality() {
        assert_eq!(PaneId::Tab(TabId::Search), PaneId::Tab(TabId::Search));
        assert_ne!(PaneId::Tab(TabId::Search), PaneId::Tab(TabId::Queue));
        assert_ne!(PaneId::Tab(TabId::Search), PaneId::Detail(DetailId::Artist));
    }

    #[test]
    fn test_tab_id_labels() {
        assert_eq!(TabId::Search.label(), "Search");
        assert_eq!(TabId::Queue.label(), "Queue");
        assert_eq!(TabId::Library.label(), "Library");
    }

    #[test]
    fn test_tab_id_hotkeys() {
        assert_eq!(TabId::Search.hotkey(), '1');
        assert_eq!(TabId::Queue.hotkey(), '2');
        assert_eq!(TabId::Library.hotkey(), '3');
    }
}
