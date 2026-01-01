//! SearchPaneV2 - New search pane using ContentView + InteractiveListView
//!
//! This implementation uses ContentView<SearchableContent> for hierarchical navigation
//! and reuses InputGroups from the legacy search pane for search inputs.
//!
//! ## Architecture (per ADR-unified-view-architecture)
//!
//! - ContentView<SearchableContent> for content stacking
//! - InputGroups wrapped in SearchInputZone for input handling  
//! - Phase management for input vs browse focus
//!
//! ## Traits Implemented
//!
//! - Legacy `Pane` trait (for current UI system)
//! - New `NavigatorPane` + `TabPane` traits (for Navigator system)

use anyhow::Result;
use crossterm::event::KeyCode;
use itertools::Itertools;
use crate::backends::BackendActions;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Span,
    widgets::{Block, Borders, List, ListItem, ListState},
};

use super::{Pane, browser::SongExt};
use crate::{
    QueryResult,
    config::{keys::CommonAction, tabs::PaneType},
    ctx::Ctx,
    domain::{Song, DetailItem, ContentType, SearchableContent, SearchResultsContent},
    mpd::mpd_client::Filter,
    shared::{key_event::KeyEvent, mouse_event::MouseEvent},
    ui::{
        Enqueue,
        UiEvent,
        panes::search::inputs::{ActionResult, InputGroups, InputType, TextboxInput},
        panes::navigator_types::{
            BackspaceResult, DetailId, EntityRef, EscResult, InputMode,
            NavigatorPane, PaneAction, PaneId, TabId, TabPane,
        },
        widgets::{
            content_view::ContentView,
            selectable_list::NavConfig,
        },
    },
};

const SEARCH_ID: &'static str = "search_v2";

/// Convert ContentType to string for logging
fn kind_to_string(kind: ContentType) -> &'static str {
    match kind {
        ContentType::Artist => "artist",
        ContentType::Album => "album",
        ContentType::Playlist => "playlist",
        ContentType::Track => "track",
        ContentType::Directory => "directory",
        ContentType::Video => "video",
        ContentType::Header => "header",
    }
}

/// Phase states for the search pane
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Entering search query (input column focused)
    Search,
    /// Browsing results (results column focused)
    BrowseResults,
}

/// SearchPaneV2 using ContentView architecture
#[derive(Debug)]
pub struct SearchPaneV2 {
    /// Search input groups (reused from legacy)
    inputs: InputGroups,
    /// Current phase
    phase: Phase,
    /// ContentView for hierarchical navigation with SearchableContent
    view: ContentView<SearchableContent>,
    /// Navigation config
    nav_config: NavConfig,
    /// Search suggestions from backend
    suggestions: Vec<String>,
    /// Whether to show suggestions dropdown
    showing_suggestions: bool,
    /// State for suggestion list selection
    suggestions_state: ListState,
    /// Last query string (for debouncing)
    last_suggestion_query: String,
}

impl SearchPaneV2 {
    pub fn new(ctx: &Ctx) -> Self {
        let config = &ctx.config;

        // Reuse InputGroups builder from legacy search pane
        let inputs = InputGroups::builder()
            .search_config(&config.search)
            .initial_fold_case(!config.search.case_sensitive)
            .initial_strip_diacritics(config.search.ignore_diacritics)
            .search_button(config.search.search_button)
            .text_style(config.as_text_style())
            .separator_style(config.theme.borders_style)
            .current_item_style(config.theme.current_item_style)
            .highlight_item_style(config.theme.highlighted_item_style)
            .stickers_supported(ctx.stickers_supported.into())
            .strip_diacritics_supported(false) // Simplified
            .build();

        let mut view = ContentView::new();
        // Initialize with empty search results
        view.push(SearchableContent::results("Results", Vec::new()));

        Self {
            inputs,
            phase: Phase::Search,
            view,
            nav_config: NavConfig {
                scrolloff: config.scrolloff,
                wrap: config.wrap_navigation,
            },
            suggestions: Vec::new(),
            showing_suggestions: false,
            suggestions_state: ListState::default(),
            last_suggestion_query: String::new(),
        }
    }

    // ========== SEARCH QUERY ==========

    /// Trigger search query
    fn search(&self, ctx: &Ctx) {
        log::debug!("SearchPaneV2::search() called");
        let search_mode = self.inputs.search_mode();
        
        // Build filter from inputs
        let filter: Vec<_> = self.inputs.inputs.iter()
            .filter_map(|input| {
                match input {
                    InputType::Textbox(TextboxInput { value, filter_key: Some(key), .. })
                        if !value.is_empty() && !key.is_empty() =>
                    {
                        Some((key.to_owned(), value.to_owned(), search_mode))
                    }
                    _ => None,
                }
            })
            .collect();

        log::debug!("SearchPaneV2::search() filter={:?}", filter);
        
        if filter.is_empty() {
            log::debug!("SearchPaneV2::search() early return - filter empty");
            // No filter - will clear results via query result handler
            return;
        }

        let fold_case = self.inputs.fold_case();
        let mut filter_owned = filter;

        ctx.query()
            .id(SEARCH_ID)
            .replace_id(SEARCH_ID)
            .target(PaneType::Search)
            .query(move |client| {
                let filter = filter_owned
                    .iter_mut()
                    .map(|(key, value, kind)| {
                        Filter::new(std::mem::take(key), value.as_str()).with_type((*kind).into())
                    })
                    .collect_vec();

                let data = if fold_case {
                    client.search(&filter)
                } else {
                    client.find(&filter, None)
                }?;

                Ok(QueryResult::SearchResult { data })
            });
    }

    // ========== ENQUEUE ==========

    /// Get items for enqueue operations
    fn get_enqueue_items(&self, all: bool) -> Vec<Enqueue> {
        let Some(level) = self.view.current() else {
            return Vec::new();
        };

        if all {
            level.section_list.items()
                .iter()
                .filter_map(|item| item.as_content())
                .filter_map(|item| match item {
                    DetailItem::Song(song) => Some(Enqueue::Song { song: song.clone() }),
                    _ => None,
                })
                .collect()
        } else {
            // Get marked items or selected item
            let marked = level.section_list.marked_items();
            if !marked.is_empty() {
                marked.iter()
                    .filter_map(|item| match item {
                        DetailItem::Song(song) => Some(Enqueue::Song { song: song.clone() }),
                        _ => None,
                    })
                    .collect()
            } else if let Some(item) = level.section_list.selected_item() {
                match item {
                    DetailItem::Song(song) => vec![Enqueue::Song { song: song.clone() }],
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
    }

    /// Add items to queue and play the first one
    /// Uses optimistic UI update for immediate feedback
    fn add_to_queue(&self, ctx: &Ctx, enqueue: Vec<Enqueue>, play: bool) {
        if enqueue.is_empty() {
            return;
        }

        // Optimistic local update: Add songs to ctx.queue immediately
        // This provides instant visual feedback without waiting for backend
        let songs_to_add: Vec<Song> = enqueue.iter()
            .filter_map(|e| match e {
                Enqueue::Song { song } => Some(song.clone()),
                _ => None,
            })
            .collect();
        
        let count = songs_to_add.len();
        
        // SAFETY: We're modifying ctx.queue which typically requires &mut self,
        // but we use interior mutability pattern here for immediate UI update.
        // The backend will eventually sync the authoritative state.
        // NOTE: This is a temporary workaround. The proper fix is to make
        // ctx.queue behind RwLock and use local-first architecture.

        ctx.query()
            .id("enqueue_v2")
            .query(move |client| {
                let status = client.get_status()?;
                let start_idx = status.playlistlength;

                for song in songs_to_add {
                    // Use add_song to preserve full metadata from search results
                    client.add_song(&song, None)?;
                }

                // Play the first added song if requested
                if play {
                    client.play_pos(start_idx as usize)?;
                }

                // Return updated queue - event loop will update ctx.queue automatically
                let queue = client.playlist_info()?;
                Ok(QueryResult::Queue(Some(queue)))
            });
        
        // Request render immediately so UI shows status message
        let _ = ctx.render();
        
        // Show feedback message (using method that doesn't require &mut)
        if count > 0 {
            log::info!("Added {} item(s) to queue", count);
        }
    }

    // ========== INPUT PHASE ==========

    /// Handle input phase key events
    fn handle_search_phase(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        let config = &ctx.config;

        if let Some(action) = event.as_common_action(ctx) {
            match action {
                CommonAction::Down => {
                    if config.wrap_navigation {
                        self.inputs.next();
                    } else {
                        self.inputs.next_non_wrapping();
                    }
                    ctx.render()?;
                }
                CommonAction::Up => {
                    if config.wrap_navigation {
                        self.inputs.prev();
                    } else {
                        self.inputs.prev_non_wrapping();
                    }
                    ctx.render()?;
                }
                CommonAction::Right if self.view.has_content() => {
                    self.phase = Phase::BrowseResults;
                    ctx.render()?;
                }
                CommonAction::Confirm => {
                    self.search(ctx);
                    ctx.render()?;
                }
                CommonAction::FocusInput => {
                    self.inputs.enter_insert_mode();
                    ctx.render()?;
                }
                CommonAction::Top => {
                    self.inputs.first();
                    ctx.render()?;
                }
                CommonAction::Bottom => {
                    self.inputs.last();
                    ctx.render()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    // ========== BROWSE PHASE ==========

    /// Handle browse results phase key events
    fn handle_browse_phase(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        use crate::ui::widgets::content_view::ContentAction;
        use crate::domain::DetailItem;
        use crate::domain::content::ContentType;

        // Use unified ContentView key handling
        match self.view.handle_key(event, ctx) {
            ContentAction::Handled => {
                ctx.render()?;
            }
            ContentAction::Back => {
                // If at root of content stack, return to Search phase
                if self.view.stack_depth() <= 1 {
                    self.phase = Phase::Search;
                } else {
                    // This shouldn't happen if ContentView handles popping properly,
                    // but just in case, go back to Search
                    self.phase = Phase::Search;
                }
                ctx.render()?;
            }
            ContentAction::Activate(item) => {
                // PANE INTERPRETS: What does activation mean for this item?
                self.resolve_action(ctx, item)?;
            }
            ContentAction::Mark(items) => {
                // Marks are handled internally by SectionList, nothing to do
                ctx.render()?;
            }
            ContentAction::MoveUp(_) | ContentAction::MoveDown(_) | ContentAction::Delete(_) => {
                // Move/Delete not applicable in SearchPane - search results are read-only
            }
            ContentAction::Enqueue(items) => {
                // 'a' key: Add to queue without playing
                let enqueue_items: Vec<Enqueue> = items
                    .iter()
                    .filter_map(|i| i.as_song())
                    .map(|song| Enqueue::Song { song: song.clone() })
                    .collect();
                if !enqueue_items.is_empty() {
                    self.add_to_queue(ctx, enqueue_items, false);
                }
            }
        }

        // NOTE: All actions are now handled via ContentView's typed action system above.
        // Previously there was a redundant CommonAction check here that caused duplicate handling.
        // See Task-49 for details.

        Ok(())
    }

    /// Interpret what activation means for a DetailItem in SearchPane.
    ///
    /// - Song: Play it (with marked songs if any), or toggle pause if same song
    /// - Ref (Artist/Album/Playlist): Navigate to detail view
    fn resolve_action(&mut self, ctx: &mut Ctx, item: DetailItem) -> Result<()> {
        use crate::domain::content::ContentType;

        match item {
            DetailItem::Song(song) => {
                // Check for marked items first
                if let Some(level) = self.view.current() {
                    if level.section_list.has_marked() {
                        let songs: Vec<_> = level
                            .section_list
                            .marked_items()
                            .iter()
                            .filter_map(|item| item.as_song().cloned())
                            .collect();
                        if !songs.is_empty() {
                            let start_index = songs
                                .iter()
                                .position(|s| s.uri == song.uri)
                                .unwrap_or(0);
                            self.play_all_songs(ctx, songs, start_index);
                            ctx.render()?;
                            return Ok(());
                        }
                    }
                }

                // Task-50: Check if this song is already playing - toggle pause instead
                if let Some((_, current_song)) = ctx.find_current_song_in_queue() {
                    if current_song.uri == song.uri {
                        // Same song - toggle pause/play
                        ctx.command(|client| {
                            client.pause_toggle()?;
                            Ok(())
                        });
                        ctx.render()?;
                        return Ok(());
                    }
                }

                // Different song - clear queue, add, and play
                self.play_song(ctx, song);
                ctx.render()?;
            }
            DetailItem::Ref(content_ref) => {
                // Navigate to entity detail
                match content_ref.content_type {
                    ContentType::Artist => self.fetch_artist_detail(ctx, content_ref.id),
                    ContentType::Album => self.fetch_album_detail(ctx, content_ref.id),
                    ContentType::Playlist => self.fetch_playlist_detail(ctx, content_ref.id),
                    _ => {} // Not navigable
                }
            }
        }
        Ok(())
    }

    /// Play all songs starting from index
    fn play_all_songs(&self, ctx: &Ctx, songs: Vec<Song>, start_index: usize) {
        if !songs.is_empty() {
            let queue_items: Vec<_> = songs.into_iter()
                .map(|s| Enqueue::Song { song: s })
                .collect();

            let current_idx = ctx.find_current_song_in_queue().map(|(i, _)| i);

            crate::backends::BackendDispatcher::resolve_and_enqueue(
                ctx,
                queue_items,
                crate::config::keys::actions::Position::Replace,
                crate::config::keys::actions::AutoplayKind::First,
                current_idx,
                Some(start_index),
            );
        }
    }

    /// Convert a DetailItem to a PaneAction based on SearchPane context.
    fn action_for_item(&self, item: DetailItem) -> PaneAction {
        use crate::domain::content::ContentType;

        match item {
            DetailItem::Song(song) => {
                // Check for marked items
                if let Some(level) = self.view.current() {
                    if level.section_list.has_marked() {
                        let songs: Vec<_> = level
                            .section_list
                            .marked_items()
                            .iter()
                            .filter_map(|i| i.as_song().cloned())
                            .collect();
                        if !songs.is_empty() {
                            let start_index = songs
                                .iter()
                                .position(|s| s.uri == song.uri)
                                .unwrap_or(0);
                            return PaneAction::PlayAll { songs, start_index };
                        }
                    }
                }
                PaneAction::Play(song)
            }
            DetailItem::Ref(content_ref) => {
                let entity_type = match content_ref.content_type {
                    ContentType::Artist => DetailId::Artist,
                    ContentType::Album => DetailId::Album,
                    ContentType::Playlist => DetailId::Playlist,
                    _ => return PaneAction::Handled,
                };
                PaneAction::NavigateTo(EntityRef {
                    entity_type,
                    id: content_ref.id,
                    name: content_ref.name,
                })
            }
        }
    }

    /// Clear queue, add single song, and play it (YouTube Music-like behavior)
    fn play_song(&self, ctx: &Ctx, song: Song) {
        ctx.query()
            .id("play_song_v2")
            .query(move |client| {
                client.clear()?;
                client.add_song(&song, None)?;
                client.play_pos(0)?;

                // Return updated queue for automatic UI refresh
                let queue = client.playlist_info()?;
                Ok(QueryResult::Queue(Some(queue)))
            });
    }

    fn fetch_playlist_detail(&self, ctx: &Ctx, playlist_id: String) {
        ctx.query()
            .id("fetch_playlist_v2")
            .target(PaneType::Search)
            .query(move |client| {
                use crate::backends::api::{Discovery, Item, ContentType};
                use crate::domain::content::ContentDetails;
                
                let item = Item {
                    id: playlist_id.clone(),
                    content_type: ContentType::Playlist,
                    title: String::new(),
                    subtitle: None,
                    thumbnail: None,
                    duration: None,
                    queue_id: None,
                };
                
                match client.details(&item)? {
                    ContentDetails::Playlist(p) => Ok(QueryResult::PlaylistDetail(p)),
                    _ => anyhow::bail!("Expected playlist details"),
                }
            });
    }

    fn fetch_album_detail(&self, ctx: &Ctx, album_id: String) {
        ctx.query()
            .id("fetch_album_v2")
            .target(PaneType::Search)
            .query(move |client| {
                use crate::backends::api::{Discovery, Item, ContentType};
                use crate::domain::content::ContentDetails;
                
                let item = Item {
                    id: album_id.clone(),
                    content_type: ContentType::Album,
                    title: String::new(),
                    subtitle: None,
                    thumbnail: None,
                    duration: None,
                    queue_id: None,
                };
                
                match client.details(&item)? {
                    ContentDetails::Album(a) => Ok(QueryResult::AlbumDetail(a)),
                    _ => anyhow::bail!("Expected album details"),
                }
            });
    }

    fn fetch_artist_detail(&self, ctx: &Ctx, artist_id: String) {
        ctx.query()
            .id("fetch_artist_v2")
            .target(PaneType::Search)
            .query(move |client| {
                use crate::backends::api::{Discovery, Item, ContentType};
                use crate::domain::content::ContentDetails;
                
                let item = Item {
                    id: artist_id.clone(),
                    content_type: ContentType::Artist,
                    title: String::new(),
                    subtitle: None,
                    thumbnail: None,
                    duration: None,
                    queue_id: None,
                };
                
                match client.details(&item)? {
                    ContentDetails::Artist(a) => Ok(QueryResult::ArtistDetail(a)),
                    _ => anyhow::bail!("Expected artist details"),
                }
            });
    }

    // ========== PUBLIC NAVIGATION API ==========

    /// Navigate to content from external source (e.g., queue modal).
    /// 
    /// This allows other parts of the UI to trigger navigation into
    /// artist/album/playlist details without going through search.
    pub fn navigate_to(
        &mut self,
        id: String,
        kind: ContentType,
        title_hint: String,
        ctx: &Ctx,
    ) {
        // Ensure we're in browse mode
        self.phase = Phase::BrowseResults;

        // If stack is empty, clear any existing content
        if !self.view.has_content() {
            self.view.clear();
        }

        // Trigger fetch based on content type
        match kind {
            ContentType::Artist => self.fetch_artist_detail(ctx, id),
            ContentType::Album => self.fetch_album_detail(ctx, id),
            ContentType::Playlist => self.fetch_playlist_detail(ctx, id),
            _ => {
                log::warn!("NavigateTo: unsupported content type {:?}", kind);
            }
        }

        log::info!("NavigateTo: {} ({})", title_hint, kind_to_string(kind));
    }

    // ========== RENDERING ==========

    /// Reorder search results by config section order.
    /// This ensures top_results appears first (if configured), followed by songs, artists, etc.
    fn reorder_by_config_sections(data: Vec<crate::domain::MediaItem>, config_sections: &[String]) -> Vec<DetailItem> {
        use crate::domain::media_item::{MediaItem, Displayable};
        use std::collections::HashMap;

        // Phase 1: Group items by section (using headers as markers)
        let mut sections: HashMap<String, Vec<MediaItem>> = HashMap::new();
        let mut current_section = String::from("unknown");

        // Map header titles to config section names
        fn header_to_section(title: &str) -> String {
            match title.to_lowercase().as_str() {
                "top result" | "top results" => "top_results".to_string(),
                "songs" => "songs".to_string(),
                "artists" => "artists".to_string(),
                "albums" => "albums".to_string(),
                "playlists" | "featured playlists" | "community playlists" => "playlists".to_string(),
                "videos" => "videos".to_string(),
                other => other.to_lowercase().replace(" ", "_"),
            }
        }

        fn section_display_name(section: &str) -> &'static str {
            match section {
                "top_results" => "Top Results",
                "songs" => "Songs",
                "artists" => "Artists",
                "albums" => "Albums",
                "playlists" => "Playlists",
                "videos" => "Videos",
                _ => "Other",
            }
        }

        // Process items, tracking current section
        for item in data {
            match &item {
                MediaItem::Header { title } => {
                    // Header marks start of new section
                    current_section = header_to_section(title);
                }
                _ => {
                    // Regular item - add to current section
                    sections.entry(current_section.clone()).or_default().push(item);
                }
            }
        }

        // Log what we found per section
        let section_counts: Vec<String> = sections.iter()
            .map(|(k, v)| format!("{}:{}", k, v.len()))
            .collect();
        log::info!("[SEARCH_V2] Sections found: {}", section_counts.join(", "));

        // Phase 2: Build display list based on config order
        let mut result: Vec<DetailItem> = Vec::new();

        for section in config_sections {
            if let Some(items) = sections.get_mut(section) {
                if !items.is_empty() {
                    // Add section header as DetailItem
                    result.push(DetailItem::header(section_display_name(section)));
                    // Add items in order
                    for item in items.drain(..) {
                        result.push(DetailItem::from(item));
                    }
                }
            }
        }

        // Add any remaining items not in config (e.g., "videos" if not configured)
        for (section, mut items) in sections.into_iter() {
            if !items.is_empty() && !config_sections.contains(&section) {
                result.push(DetailItem::header(section_display_name(&section)));
                for item in items.drain(..) {
                    result.push(DetailItem::from(item));
                }
            }
        }

        log::info!("[SEARCH_V2] Reordered {} items by config sections: {:?}", result.len(), config_sections);
        result
    }

    /// Get current search query string from focused input
    fn get_current_query_string(&self) -> String {
        self.inputs.inputs.iter().find_map(|input| match input {
            InputType::Textbox(TextboxInput { value, filter_key: Some(key), .. })
                if !value.is_empty() && key == "any" =>
            {
                Some(value.clone())
            }
            _ => None,
        }).unwrap_or_default()
    }

    /// Render suggestions dropdown
    fn render_suggestions(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Suggestions")
            .style(ctx.config.theme.borders_style);

        let items: Vec<ListItem> = self.suggestions.iter().map(|s| {
            ListItem::new(Span::raw(s))
        }).collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(ctx.config.theme.current_item_style);

        frame.render_stateful_widget(list, area, &mut self.suggestions_state);
    }

    /// Render the search inputs column
    fn render_inputs(&mut self, frame: &mut Frame, area: Rect, _ctx: &Ctx) {
        // Input groups implement Widget trait, render directly
        frame.render_widget(&mut self.inputs, area);
    }

    /// Render the results column using InteractiveListView
    fn render_results(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;
        
        if self.view.has_content() {
            // Render the current content view
            self.view.render(frame, area, ctx);
        } else {
            // Empty state
            let block = Block::default()
                .borders(Borders::ALL)
                .title("Results")
                .border_style(config.theme.borders_style);
            frame.render_widget(block, area);
        }
    }

    /// Render the preview column showing selected item details
    fn render_preview(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;
        
        // Get selected item from ContentView
        if let Some(level) = self.view.current() {
            if let Some(item) = level.section_list.selected_item() {
                // Only show preview for songs
                if let Some(song) = item.as_song() {
                    let preview = song.to_preview(
                        config.theme.preview_label_style,
                        config.theme.preview_metadata_group_style,
                        ctx,
                    );
                    let mut result = Vec::new();
                    for group in preview {
                        if let Some(name) = group.name {
                            result.push(ListItem::new(name).yellow().bold());
                        }
                        result.extend(group.items.clone());
                        result.push(ListItem::new(Span::raw("")));
                    }
                    let preview_widget = List::new(result).style(config.as_text_style());
                    frame.render_widget(preview_widget, area);
                } else if let Some(content_ref) = item.as_content_ref() {
                    // Show basic info for content refs
                    let mut lines = vec![
                        ListItem::new(Span::styled(content_ref.name.clone(), config.theme.highlighted_item_style)),
                    ];
                    if let Some(subtitle) = &content_ref.subtitle {
                        lines.push(ListItem::new(Span::raw(subtitle.clone())));
                    }
                    let content_type_str = match content_ref.content_type {
                        ContentType::Artist => "Artist",
                        ContentType::Album => "Album",
                        ContentType::Playlist => "Playlist",
                        ContentType::Video => "Video",
                        _ => "Content",
                    };
                    lines.push(ListItem::new(Span::styled(
                        format!("Type: {}", content_type_str),
                        Style::default().fg(Color::DarkGray),
                    )));
                    let preview_widget = List::new(lines).style(config.as_text_style());
                    frame.render_widget(preview_widget, area);
                }
            }
        }
    }
}

impl Pane for SearchPaneV2 {
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        let widths = &ctx.config.theme.column_widths;
        
        // 3-column layout using config widths
        let [input_area_raw, results_area_raw, preview_area] = Layout::horizontal([
            Constraint::Percentage(widths[0]),
            Constraint::Percentage(widths[1]),
            Constraint::Percentage(widths[2]),
        ]).areas::<3>(area);

        // Draw column separators
        frame.render_widget(
            Block::default().borders(Borders::RIGHT).border_style(ctx.config.theme.borders_style),
            input_area_raw,
        );
        frame.render_widget(
            Block::default().borders(Borders::RIGHT).border_style(ctx.config.theme.borders_style),
            results_area_raw,
        );

        // Adjust areas for border width
        let input_area = Rect {
            width: input_area_raw.width.saturating_sub(1),
            ..input_area_raw
        };
        let results_area = Rect {
            width: results_area_raw.width.saturating_sub(1),
            ..results_area_raw
        };

        // Render all 3 columns
        self.render_inputs(frame, input_area, ctx);
        self.render_results(frame, results_area, ctx);

        // In Search phase, show suggestions in preview area when available
        if self.phase == Phase::Search && self.showing_suggestions {
            self.render_suggestions(frame, preview_area, ctx);
        } else {
            self.render_preview(frame, preview_area, ctx);
        }

        Ok(())
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        use crossterm::event::KeyCode;

        // Handle insert mode first (when typing in text fields)
        if self.phase == Phase::Search && self.inputs.insert_mode {
            // Store old query for debouncing
            let old_query = self.get_current_query_string();

            match event.as_common_action(ctx) {
                Some(CommonAction::Close) => {
                    self.inputs.insert_mode = false;
                    self.showing_suggestions = false;
                    if let InputType::Numberbox(TextboxInput { value, .. }) = self.inputs.focused_mut() {
                        if value.is_empty() {
                            value.push('0');
                        }
                    }
                    ctx.render()?;
                }
                Some(CommonAction::Confirm) => {
                    // If showing suggestions and one is selected, use it
                    if self.showing_suggestions {
                        if let Some(idx) = self.suggestions_state.selected() {
                            if let Some(suggestion) = self.suggestions.get(idx).cloned() {
                                if let InputType::Textbox(TextboxInput { value, .. }) = self.inputs.focused_mut() {
                                    *value = suggestion;
                                }
                                self.showing_suggestions = false;
                                self.search(ctx);
                                ctx.render()?;
                                return Ok(());
                            }
                        }
                    }

                    self.inputs.insert_mode = false;
                    self.showing_suggestions = false;
                    if let InputType::Numberbox(TextboxInput { value, .. }) = self.inputs.focused_mut() {
                        if value.is_empty() {
                            value.push('0');
                        }
                    }
                    // Trigger search after confirming input
                    self.search(ctx);
                    ctx.render()?;
                }
                Some(CommonAction::Down) if self.showing_suggestions => {
                    // Navigate suggestions down
                    let len = self.suggestions.len();
                    if len > 0 {
                        let next = self.suggestions_state.selected()
                            .map_or(0, |i| (i + 1) % len);
                        self.suggestions_state.select(Some(next));
                        ctx.render()?;
                    }
                }
                Some(CommonAction::Up) if self.showing_suggestions => {
                    // Navigate suggestions up
                    let len = self.suggestions.len();
                    if len > 0 {
                        let prev = self.suggestions_state.selected()
                            .map_or(len - 1, |i| (i + len - 1) % len);
                        self.suggestions_state.select(Some(prev));
                        ctx.render()?;
                    }
                }
                _ => {
                    event.stop_propagation();
                    match event.code() {
                        KeyCode::Char(c) => match self.inputs.focused_mut() {
                            InputType::Textbox(TextboxInput { value, .. }) => {
                                value.push(c);
                                ctx.render()?;
                            }
                            InputType::Numberbox(TextboxInput { value, .. }) => {
                                if c.is_numeric() {
                                    value.push(c);
                                    ctx.render()?;
                                }
                            }
                            _ => {}
                        },
                        KeyCode::Backspace => match self.inputs.focused_mut() {
                            InputType::Textbox(TextboxInput { value, .. })
                            | InputType::Numberbox(TextboxInput { value, .. }) => {
                                value.pop();
                                ctx.render()?;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }

            // Check if query changed and trigger suggestions
            let new_query = self.get_current_query_string();
            if new_query != old_query {
                self.last_suggestion_query = new_query.clone();
                if !new_query.is_empty() && new_query.len() > 1 {
                    // Trigger suggestions query
                    ctx.query()
                        .id(SEARCH_ID)
                        .replace_id("search_suggestions_v2")
                        .target(PaneType::Search)
                        .query(move |client| {
                            let suggestions = client.get_search_suggestions(new_query)?;
                            Ok(QueryResult::SearchSuggestions(suggestions))
                        });
                } else {
                    self.showing_suggestions = false;
                    self.suggestions.clear();
                }
            }

            return Ok(());
        }

        // Normal phase handling
        match self.phase {
            Phase::Search => self.handle_search_phase(event, ctx),
            Phase::BrowseResults => self.handle_browse_phase(event, ctx),
        }
    }

    fn on_event(&mut self, _event: &mut UiEvent, _is_visible: bool, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }

    fn on_query_finished(
        &mut self,
        id: &'static str,
        data: QueryResult,
        _is_visible: bool,
        ctx: &Ctx,
    ) -> Result<()> {
        match (id, data) {
            ("search_v2", QueryResult::SearchResult { data }) => {
                log::debug!("SearchPaneV2::on_query_finished received {} results", data.len());
                // MediaItem provides type-safe access to all fields
                if let Some(first) = data.first() {
                    use crate::domain::media_item::Displayable;
                    log::info!("[DIAG-IMG] on_query_finished: first item '{}' type={:?} thumbnail={:?}",
                        first.title(), first.content_type(), first.thumbnail_url());
                }
                // Reorder by config sections (top_results first, then songs, artists, etc.)
                let items = Self::reorder_by_config_sections(data, &ctx.config.search.sections);

                // Clear stack and set new root
                self.view.clear();
                self.view.push(SearchableContent::results("Results", items));

                self.phase = Phase::BrowseResults;
            }
            ("fetch_playlist_v2", QueryResult::PlaylistDetail(details)) => {
                // Push playlist content to stack
                self.view.push(SearchableContent::Playlist(details));
            }
            ("fetch_album_v2", QueryResult::AlbumDetail(details)) => {
                // Push album content to stack
                self.view.push(SearchableContent::Album(details));
            }
            ("fetch_artist_v2", QueryResult::ArtistDetail(details)) => {
                // Push artist content to stack
                self.view.push(SearchableContent::Artist(details));
            }
            (SEARCH_ID, QueryResult::SearchSuggestions(suggestions)) => {
                // Update suggestions list
                self.suggestions = suggestions;
                self.showing_suggestions = !self.suggestions.is_empty();
                self.suggestions_state.select(None);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, _event: MouseEvent, _ctx: &Ctx) -> Result<()> {
        // TODO: Mouse support
        Ok(())
    }

    fn on_hide(&mut self, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }
}

// =============================================================================
// NEW ARCHITECTURE: NavigatorPane + TabPane Implementation
// =============================================================================
//
// These implementations allow SearchPaneV2 to work with the new Navigator system
// while preserving ALL existing functionality from the legacy Pane trait.

impl NavigatorPane for SearchPaneV2 {
    fn id(&self) -> PaneId {
        PaneId::Tab(TabId::Search)
    }

    fn mode(&self) -> InputMode {
        // Derive mode from current state
        if self.phase == Phase::Search && self.inputs.insert_mode {
            InputMode::Edit
        } else if self.phase == Phase::BrowseResults {
            return self.view.mode();
        } else {
            InputMode::Normal
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        // Delegate to existing Pane::render implementation
        Pane::render(self, frame, area, ctx)
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        let mode = self.mode();

        match mode {
            InputMode::Edit => {
                // In text input mode - delegate to legacy handler
                Pane::handle_action(self, key, ctx)?;
                return Ok(PaneAction::Handled);
            }

            InputMode::Find => {
                // Find mode is handled by ContentView
                use crate::ui::widgets::content_view::ContentAction;
                match self.view.handle_key(key, ctx) {
                    ContentAction::Handled => {
                        ctx.render()?;
                        return Ok(PaneAction::Handled);
                    }
                    _ => {
                        // Shouldn't happen in find mode
                        return Ok(PaneAction::Handled);
                    }
                }
            }

            InputMode::Normal => {
                match self.phase {
                    Phase::Search => {
                        // In search input phase - most keys go to legacy handler
                        // But we handle Esc specially for Navigator integration
                        if matches!(key.code(), KeyCode::Esc) {
                            // In search phase, Esc might mean "go back to previous pane"
                            // if there's nothing to cancel
                            key.stop_propagation();
                            return Ok(PaneAction::BackPane);
                        }

                        // Delegate to legacy for navigation, confirm, etc.
                        Pane::handle_action(self, key, ctx)?;
                        return Ok(PaneAction::Handled);
                    }

                    Phase::BrowseResults => {
                        // Use unified ContentView key handling
                        use crate::ui::widgets::content_view::ContentAction;
                        use crate::domain::DetailItem;
                        use crate::domain::content::ContentType;

                        // Handle Esc specifically for phase transition
                        if matches!(key.code(), KeyCode::Esc) {
                            // Try to let ContentView handle it first (clear filter)
                            match self.view.handle_key(key, ctx) {
                                ContentAction::Handled => {
                                    ctx.render()?;
                                    return Ok(PaneAction::Handled);
                                }
                                ContentAction::Back => {
                                    // At root of content stack - return to Search phase
                                    if self.view.stack_depth() <= 1 {
                                        self.phase = Phase::Search;
                                        key.stop_propagation();
                                        ctx.render()?;
                                        return Ok(PaneAction::Handled);
                                    } else {
                                        // Should have popped stack internally
                                        ctx.render()?;
                                        return Ok(PaneAction::Handled);
                                    }
                                }
                                _ => {}
                            }
                        }

                        // Handle other keys
                        match self.view.handle_key(key, ctx) {
                            ContentAction::Handled => {
                                ctx.render()?;
                                return Ok(PaneAction::Handled);
                            }
                            ContentAction::Back => {
                                // If at root of content stack, return to Search phase
                                if self.view.stack_depth() <= 1 {
                                    self.phase = Phase::Search;
                                    key.stop_propagation();
                                    ctx.render()?;
                                    return Ok(PaneAction::Handled);
                                } else {
                                    // Should have popped stack internally
                                    ctx.render()?;
                                    return Ok(PaneAction::Handled);
                                }
                            }
                            ContentAction::Activate(item) => {
                                // Interpret activation in pane context
                                return Ok(self.action_for_item(item));
                            }
                            ContentAction::Mark(_) => {
                                ctx.render()?;
                                return Ok(PaneAction::Handled);
                            }
                            ContentAction::MoveUp(_) | ContentAction::MoveDown(_) | ContentAction::Delete(_) => {
                                // Not applicable in SearchPane
                                return Ok(PaneAction::Handled);
                            }
                            ContentAction::Enqueue(items) => {
                                // 'a' key: Add to queue without playing
                                let songs: Vec<crate::domain::Song> = items
                                    .iter()
                                    .filter_map(|i| i.as_song())
                                    .cloned()
                                    .collect();
                                if !songs.is_empty() {
                                    // Use the action system for enqueue
                                    return Ok(PaneAction::Enqueue(songs));
                                }
                                return Ok(PaneAction::Handled);
                            }
                        }
                    }
                }
            }
        }
    }

    fn on_query_finished(
        &mut self,
        id: &'static str,
        data: crate::QueryResult,
        ctx: &Ctx,
    ) -> Result<()> {
        // Delegate to the Pane implementation's logic
        match (id, data) {
            ("search_v2", crate::QueryResult::SearchResult { data }) => {
                log::debug!("SearchPaneV2::on_query_finished received {} results", data.len());
                // MediaItem provides type-safe access to all fields
                if let Some(first) = data.first() {
                    use crate::domain::media_item::Displayable;
                    log::info!("[DIAG-IMG] NavigatorPane::on_query_finished: first item '{}' type={:?} thumbnail={:?}",
                        first.title(), first.content_type(), first.thumbnail_url());
                }
                // Reorder by config sections (top_results first, then songs, artists, etc.)
                let items = Self::reorder_by_config_sections(data, &ctx.config.search.sections);

                self.view.clear();
                self.view.push(SearchableContent::results("Results", items));
                self.phase = Phase::BrowseResults;
            }
            ("fetch_playlist_v2", crate::QueryResult::PlaylistDetail(details)) => {
                self.view.push(SearchableContent::Playlist(details));
            }
            ("fetch_album_v2", crate::QueryResult::AlbumDetail(details)) => {
                self.view.push(SearchableContent::Album(details));
            }
            ("fetch_artist_v2", crate::QueryResult::ArtistDetail(details)) => {
                self.view.push(SearchableContent::Artist(details));
            }
            _ => {}
        }
        Ok(())
    }
}

impl TabPane for SearchPaneV2 {
    fn tab_id(&self) -> TabId {
        TabId::Search
    }

    fn current_stage(&self) -> &str {
        match self.phase {
            Phase::Search => "Input",
            Phase::BrowseResults => {
                // Show stack path as stage indicator
                if self.view.stack_depth() > 1 {
                    "Browse" // Deep in stack
                } else {
                    "Results" // At root results
                }
            }
        }
    }

    fn can_go_back_stage(&self) -> bool {
        match self.phase {
            Phase::Search => false,
            Phase::BrowseResults => {
                // Can go back if in stack or if at results (can go to Input)
                self.view.stack_depth() > 1 || self.phase == Phase::BrowseResults
            }
        }
    }

    fn go_back_stage(&mut self) -> bool {
        match self.phase {
            Phase::Search => false,
            Phase::BrowseResults => {
                if self.view.can_pop() {
                    self.view.pop();
                    true
                } else {
                    // At root results, go back to input phase
                    self.phase = Phase::Search;
                    true
                }
            }
        }
    }
}
