//! New pane architecture: TabPane + DetailPane with Navigator
//!
//! This module provides the new UI architecture with:
//! - TabPane: Panes with dedicated tabs (Search, Queue, Library)
//! - DetailPane: Entity detail panes with content stacking (Artist, Album, Playlist)
//! - Navigator: Central controller for pane history and navigation
//!
//! ## Coexistence with Legacy Panes
//!
//! The legacy `Pane` trait in `mod.rs` has different methods (handle_action, on_event, etc.)
//! This module defines `NavigatorPane` to avoid name collision.
//!
//! Migration strategy:
//! 1. New panes implement NavigatorPane + TabPane/DetailPane
//! 2. Adapters wrap existing panes (QueuePaneV2, SearchPaneV2) to provide NavigatorPane
//! 3. Navigator orchestrates NavigatorPane instances
//! 4. Eventually, all panes migrate to NavigatorPane

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

// Re-export MoveDirection for unified queue move operations
pub use crate::ui::list_ops::MoveDirection;

use crate::{
    ctx::Ctx,
    domain::Song,
    shared::key_event::KeyEvent,
    QueryResult,
};

use super::UiEvent;

// ============================================================================
// Pane Identification
// ============================================================================

/// Unique identifier for any pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    Tab(TabId),
    Detail(DetailId),
}

/// Identifier for TabPanes (have dedicated tabs, always available)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    Search,
    Queue,
    Library,
}

impl TabId {
    pub fn label(&self) -> &'static str {
        match self {
            TabId::Search => "Search",
            TabId::Queue => "Queue",
            TabId::Library => "Library",
        }
    }

    pub fn hotkey(&self) -> char {
        match self {
            TabId::Search => '1',
            TabId::Queue => '2',
            TabId::Library => '3',
        }
    }
}

/// Identifier for DetailPanes (no dedicated tab, shown when content exists)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailId {
    Artist,
    Album,
    Playlist,
}

// ============================================================================
// Input Modes
// ============================================================================

/// Input mode within a pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,  // Navigation mode
    Edit,    // Typing in text input (e.g., search box)
    Find,    // Typing find query (/)
}

// ============================================================================
// List Actions (Layer 0: InteractiveListView)
// ============================================================================

/// Actions returned from InteractiveListView key handling
#[derive(Debug, Clone)]
pub enum ListAction {
    /// Key was handled internally
    Handled,
    /// Enter pressed - activate item at index
    Activate(usize),
    /// Space pressed - return marked indices
    Mark(Vec<usize>),
    /// Shift+K - move items up
    MoveUp(Vec<usize>),
    /// Shift+J - move items down
    MoveDown(Vec<usize>),
    /// 'd' pressed - delete items
    Delete(Vec<usize>),
    /// Esc with nothing to clear - bubble up
    Back,
    /// Key not handled - pass to next layer
    Passthrough,
}

// ============================================================================
// Section Actions (Layer 1: SectionList)
// ============================================================================

use crate::domain::DetailItem;

/// Actions returned from SectionList key handling
#[derive(Debug, Clone)]
pub enum SectionAction {
    /// Key was handled internally
    Handled,
    /// Enter pressed - activate item
    Activate(DetailItem),
    /// Space pressed - return marked items
    Mark(Vec<DetailItem>),
    /// Shift+K - move items up
    MoveUp(Vec<DetailItem>),
    /// Shift+J - move items down
    MoveDown(Vec<DetailItem>),
    /// 'd' pressed - delete items
    Delete(Vec<DetailItem>),
    /// Esc/Backspace with nothing to handle - bubble up
    Back,
    /// Key not handled
    Passthrough,
}

// ============================================================================
// Content Actions (Layer 2: ContentView)
// ============================================================================

/// Actions returned from ContentView key handling
#[derive(Debug, Clone)]
pub enum ContentAction {
    /// Key was handled internally
    Handled,
    /// Enter pressed - pane interprets (play? navigate? drill?)
    Activate(DetailItem),
    /// Space pressed - return marked items
    Mark(Vec<DetailItem>),
    /// Shift+K - move items up
    MoveUp(Vec<DetailItem>),
    /// Shift+J - move items down
    MoveDown(Vec<DetailItem>),
    /// 'd' pressed - delete items
    Delete(Vec<DetailItem>),
    /// Back requested - pane decides what to do
    Back,
}

// ============================================================================
// Pane Actions
// ============================================================================

/// Actions returned from pane key handling
#[derive(Debug, Clone)]
pub enum PaneAction {
    /// Key was handled internally, no further action needed
    Handled,
    
    /// Navigate to an entity (push onto current pane's stack or switch pane)
    NavigateTo(EntityRef),
    
    /// Go back to previous pane (Esc in Normal mode)
    BackPane,
    
    /// Play a single song
    Play(Song),
    
    /// Play all songs starting from index
    PlayAll { songs: Vec<Song>, start_index: usize },
    
    /// Add songs to queue
    Enqueue(Vec<Song>),
    
    /// Queue: Delete items by queue ID
    QueueDelete(Vec<u32>),
    
    /// Queue: Move items up by one position (deprecated, use QueueMove)
    #[deprecated(note = "Use QueueMove with MoveDirection::Up")]
    QueueMoveUp(Vec<u32>),
    
    /// Queue: Move items down by one position (deprecated, use QueueMove)
    #[deprecated(note = "Use QueueMove with MoveDirection::Down")]
    QueueMoveDown(Vec<u32>),
    
    /// Queue: Move items in specified direction (unified facade)
    QueueMove { ids: Vec<u32>, direction: MoveDirection },
    
    /// Show a modal
    ShowModal(ModalKind),
    
    /// Search with query
    Search(String),
}

/// Types of modals that can be shown
#[derive(Debug, Clone)]
pub enum ModalKind {
    /// Sort queue by criteria
    Sort,
    /// Jump to position in queue
    Jump,
    /// Confirm action
    Confirm { title: String, message: String },
}

/// Reference to an entity for navigation
#[derive(Debug, Clone)]
pub struct EntityRef {
    pub entity_type: DetailId,
    pub id: String,
    pub name: String,
}

// ============================================================================
// Base Pane Trait (NavigatorPane to avoid collision with legacy Pane)
// ============================================================================

/// Common interface for all panes in the Navigator system.
///
/// Named `NavigatorPane` to avoid collision with the legacy `Pane` trait.
/// This trait focuses on navigation-aware rendering and key handling.
pub(crate) trait NavigatorPane {
    /// Get the pane's unique identifier
    fn id(&self) -> PaneId;
    
    /// Get current input mode
    fn mode(&self) -> InputMode;
    
    /// Render the pane
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()>;
    
    /// Handle a key event, returning an action for the navigator
    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction>;
    
    /// Handle UI events (queue changes, playback state, etc.)
    #[allow(unused_variables)]
    fn on_event(&mut self, event: &mut UiEvent, ctx: &Ctx) -> Result<()> {
        Ok(())
    }
    
    /// Handle async query results
    #[allow(unused_variables)]
    fn on_query_finished(&mut self, id: &'static str, data: QueryResult, ctx: &Ctx) -> Result<()> {
        Ok(())
    }
}

// ============================================================================
// TabPane Trait
// ============================================================================

/// Pane with a dedicated tab (Search, Queue, Library)
pub(crate) trait TabPane: NavigatorPane {
    /// Get the tab identifier
    fn tab_id(&self) -> TabId;
    
    /// Get the tab label for display
    fn tab_label(&self) -> &str {
        self.tab_id().label()
    }
    
    /// Get the hotkey for this tab
    fn hotkey(&self) -> char {
        self.tab_id().hotkey()
    }
    
    /// Get current stage name (for display)
    fn current_stage(&self) -> &str;
    
    /// Can go back to previous stage within this pane?
    fn can_go_back_stage(&self) -> bool;
    
    /// Go back to previous stage. Returns true if successful.
    fn go_back_stage(&mut self) -> bool;
}

// ============================================================================
// DetailPane Trait
// ============================================================================

/// Entity content that can be pushed onto a DetailPane
#[derive(Debug, Clone)]
pub enum EntityContent {
    Artist(crate::domain::ArtistContent),
    Album(crate::domain::AlbumContent),
    Playlist(crate::domain::PlaylistContent),
}

impl EntityContent {
    pub fn detail_id(&self) -> DetailId {
        match self {
            EntityContent::Artist(_) => DetailId::Artist,
            EntityContent::Album(_) => DetailId::Album,
            EntityContent::Playlist(_) => DetailId::Playlist,
        }
    }
    
    pub fn title(&self) -> &str {
        match self {
            EntityContent::Artist(a) => &a.name,
            EntityContent::Album(a) => &a.title,
            EntityContent::Playlist(p) => &p.title,
        }
    }
}

/// Pane for displaying entity details with content stacking
pub(crate) trait DetailPane: NavigatorPane {
    /// Get the detail identifier
    fn detail_id(&self) -> DetailId;
    
    /// Does this pane have any content?
    fn has_content(&self) -> bool;
    
    /// Get the current stack depth
    fn stack_depth(&self) -> usize;
    
    /// Push content onto the stack
    fn push(&mut self, content: EntityContent);
    
    /// Pop content from the stack. Returns false if at bottom (single item).
    fn pop(&mut self) -> bool;
    
    /// Clear all content from the stack
    fn clear(&mut self);
    
    /// Can go back internally (pop stack)?
    fn can_go_back_internal(&self) -> bool {
        self.stack_depth() > 1
    }
    
    /// Go back internally (pop stack). Returns true if successful.
    fn go_back_internal(&mut self) -> bool {
        self.pop()
    }
    
    /// Get the title of the current content (for breadcrumb)
    fn current_title(&self) -> Option<&str>;
}

// ============================================================================
// Esc/Backspace Result Types
// ============================================================================

/// Result of handling Esc key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscResult {
    /// Esc was handled (mode exit or find clear)
    Handled,
    /// Esc should trigger back to previous pane
    BackPane,
}

/// Result of handling Backspace key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackspaceResult {
    /// Backspace was handled (char delete or internal back)
    Handled,
    /// Backspace had no effect (at bottom of stack/stage)
    NoEffect,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_id_construction() {
        let tab_pane = PaneId::Tab(TabId::Search);
        let detail_pane = PaneId::Detail(DetailId::Artist);
        
        assert_ne!(tab_pane, detail_pane);
    }

    #[test]
    fn test_tab_id_all_values() {
        let tabs = [TabId::Search, TabId::Queue, TabId::Library];
        let labels = ["Search", "Queue", "Library"];
        let hotkeys = ['1', '2', '3'];
        
        for (i, tab) in tabs.iter().enumerate() {
            assert_eq!(tab.label(), labels[i]);
            assert_eq!(tab.hotkey(), hotkeys[i]);
        }
    }

    #[test]
    fn test_detail_id_all_values() {
        let details = [DetailId::Artist, DetailId::Album, DetailId::Playlist];
        
        // Ensure all are distinct
        for i in 0..details.len() {
            for j in (i + 1)..details.len() {
                assert_ne!(details[i], details[j]);
            }
        }
    }

    #[test]
    fn test_input_mode_default() {
        let mode = InputMode::default();
        assert_eq!(mode, InputMode::Normal);
    }

    #[test]
    fn test_entity_ref_construction() {
        let entity = EntityRef {
            entity_type: DetailId::Artist,
            id: "artist123".to_string(),
            name: "Test Artist".to_string(),
        };
        
        assert_eq!(entity.entity_type, DetailId::Artist);
        assert_eq!(entity.id, "artist123");
        assert_eq!(entity.name, "Test Artist");
    }

    #[test]
    fn test_esc_result_values() {
        assert_ne!(EscResult::Handled, EscResult::BackPane);
    }

    #[test]
    fn test_backspace_result_values() {
        assert_ne!(BackspaceResult::Handled, BackspaceResult::NoEffect);
    }
}
