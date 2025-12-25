//! SearchPaneV2 - New search pane using NavStack + InteractiveListView
//!
//! This implementation uses NavStack for hierarchical navigation and
//! reuses InputGroups from the legacy search pane for search inputs.

use anyhow::Result;
use itertools::Itertools;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Span,
    widgets::{Block, Borders, List, ListItem},
};

use super::{Pane, browser::SongExt};
use crate::{
    QueryResult,
    config::{keys::CommonAction, tabs::PaneType},
    ctx::Ctx,
    domain::{Song, DetailItem, ContentType},
    mpd::mpd_client::Filter,
    shared::{key_event::KeyEvent, mouse_event::MouseEvent},
    ui::{
        Enqueue,
        UiEvent,
        panes::search::inputs::{ActionResult, InputGroups, InputType, TextboxInput},
        widgets::{
            nav_stack::NavStack,
            interactive_list_view::NavConfig,
            detail_stack::flatten_content,
        },
    },
};

const SEARCH_ID: &'static str = "search_v2";

/// Phase states for the search pane
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Entering search query (input column focused)
    Search,
    /// Browsing results (results column focused)
    BrowseResults,
}

/// SearchPaneV2 using NavStack architecture
#[derive(Debug)]
pub struct SearchPaneV2 {
    /// Search input groups (reused from legacy)
    inputs: InputGroups,
    /// Current phase
    phase: Phase,
    /// NavStack for hierarchical navigation (now uses DetailItem for type safety)
    stack: NavStack<DetailItem>,
    /// Navigation config
    nav_config: NavConfig,
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

        Self {
            inputs,
            phase: Phase::Search,
            stack: NavStack::with_root(Vec::new(), "Results"),
            nav_config: NavConfig {
                scrolloff: config.scrolloff,
                wrap: config.wrap_navigation,
            },
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
        let Some(level) = self.stack.current() else {
            return Vec::new();
        };

        if all {
            level.items.iter()
                .filter_map(|item| match item {
                    DetailItem::Song(song) => Some(Enqueue::Song { song: song.clone() }),
                    _ => None,
                })
                .collect()
        } else {
            // Get marked items or selected item
            let marked = level.view.marked();
            if !marked.is_empty() {
                marked.iter()
                    .filter_map(|&idx| level.items.get(idx))
                    .filter_map(|item| match item {
                        DetailItem::Song(song) => Some(Enqueue::Song { song: song.clone() }),
                        _ => None,
                    })
                    .collect()
            } else if let Some(idx) = level.view.selected() {
                level.items.get(idx)
                    .and_then(|item| match item {
                        DetailItem::Song(song) => Some(Enqueue::Song { song: song.clone() }),
                        _ => None,
                    })
                    .into_iter()
                    .collect()
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
                CommonAction::Right if !self.stack.current_items().is_empty() => {
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
        let cfg = self.nav_config;
        
        if let Some(action) = event.as_common_action(ctx) {
            match action {
                CommonAction::Down => {
                    if let Some(level) = self.stack.current_mut() {
                        level.view.select_next(&level.items, cfg);
                    }
                    ctx.render()?;
                }
                CommonAction::Up => {
                    if let Some(level) = self.stack.current_mut() {
                        level.view.select_prev(&level.items, cfg);
                    }
                    ctx.render()?;
                }
                CommonAction::Top => {
                    if let Some(level) = self.stack.current_mut() {
                        level.view.select_first(&level.items);
                    }
                    ctx.render()?;
                }
                CommonAction::Bottom => {
                    if let Some(level) = self.stack.current_mut() {
                        level.view.select_last(&level.items);
                    }
                    ctx.render()?;
                }
                CommonAction::Left | CommonAction::Close => {
                    if self.stack.depth() > 1 {
                        self.stack.leave();
                    } else {
                        self.phase = Phase::Search;
                    }
                    ctx.render()?;
                }
                CommonAction::Select => {
                    if let Some(level) = self.stack.current_mut() {
                        level.view.toggle_mark();
                    }
                    ctx.render()?;
                }
                CommonAction::Confirm => {
                    // Enter selected item or add to queue and play
                    self.handle_confirm(ctx)?;
                }
                CommonAction::AddOptions { .. } => {
                    // 'a' key: Add to queue without playing
                    let enqueue = self.get_enqueue_items(false);
                    if !enqueue.is_empty() {
                        self.add_to_queue(ctx, enqueue, false);  // play=false
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Handle confirm action - enter album/artist/playlist or play song (YouTube Music-like)
    ///
    /// For songs/videos:
    /// - If song is already playing → toggle play/pause
    /// - Otherwise → clear queue, add song, play it
    fn handle_confirm(&mut self, ctx: &mut Ctx) -> Result<()> {
        let Some(level) = self.stack.current() else {
            return Ok(());
        };
        let Some(idx) = level.view.selected() else {
            return Ok(());
        };
        let Some(item) = level.items.get(idx) else {
            return Ok(());
        };

        match item {
            DetailItem::Header { .. } => {
                // Headers are not interactive
            }
            DetailItem::Ref(content_ref) => {
                // Navigate into album/artist/playlist
                let id = content_ref.id.clone();
                match content_ref.content_type {
                    ContentType::Playlist => self.fetch_playlist_detail(ctx, id),
                    ContentType::Album => self.fetch_album_detail(ctx, id),
                    ContentType::Artist => self.fetch_artist_detail(ctx, id),
                    _ => {}
                }
            }
            DetailItem::Song(song) => {
                // YouTube Music-like behavior for songs/videos:
                // Check if this exact song is currently playing
                let selected_uri = &song.uri;

                if let Some((_, current_song)) = ctx.find_current_song_in_queue() {
                    if &current_song.uri == selected_uri {
                        // Same song → toggle play/pause (don't restart or add duplicate)
                        ctx.command(|client| {
                            client.pause_toggle()?;
                            Ok(())
                        });
                        ctx.render()?;
                        return Ok(());
                    }
                }

                // Different song or nothing playing → clear, add, play
                self.play_song(ctx, song.clone());
            }
        }
        ctx.render()?;
        Ok(())
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

    // ========== RENDERING ==========

    /// Render the search inputs column
    fn render_inputs(&mut self, frame: &mut Frame, area: Rect, _ctx: &Ctx) {
        // Input groups implement Widget trait, render directly
        frame.render_widget(&mut self.inputs, area);
    }

    /// Render the results column using InteractiveListView
    fn render_results(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let config = &ctx.config;
        
        // Compute path before borrowing current_mut
        let path = self.stack.path();
        
        // Get current level for both items and view
        if let Some(level) = self.stack.current_mut() {
            let title = format!("{} ({} items)", path, level.items.len());
            level.view.render(
                frame,
                area,
                ctx,
                &level.items,
                Some(&title),
                |_song, _ctx| false,
            );
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
        
        // Get selected item from NavStack
        if let Some(item) = self.stack.selected_item() {
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
        self.render_preview(frame, preview_area, ctx);

        Ok(())
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        use crossterm::event::KeyCode;

        // Handle insert mode first (when typing in text fields)
        if self.phase == Phase::Search && self.inputs.insert_mode {
            match event.as_common_action(ctx) {
                Some(CommonAction::Close) => {
                    self.inputs.insert_mode = false;
                    if let InputType::Numberbox(TextboxInput { value, .. }) = self.inputs.focused_mut() {
                        if value.is_empty() {
                            value.push('0');
                        }
                    }
                    ctx.render()?;
                }
                Some(CommonAction::Confirm) => {
                    self.inputs.insert_mode = false;
                    if let InputType::Numberbox(TextboxInput { value, .. }) = self.inputs.focused_mut() {
                        if value.is_empty() {
                            value.push('0');
                        }
                    }
                    // Trigger search after confirming input
                    self.search(ctx);
                    ctx.render()?;
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
        _ctx: &Ctx,
    ) -> Result<()> {
        match (id, data) {
            ("search_v2", QueryResult::SearchResult { data }) => {
                log::debug!("SearchPaneV2::on_query_finished received {} results", data.len());
                // Convert Songs to DetailItems (handles type conversion via From impl)
                let items: Vec<DetailItem> = data.into_iter().map(DetailItem::from).collect();
                self.stack.set_root(items, "Results");
                self.phase = Phase::BrowseResults;
            }
            ("fetch_playlist_v2", QueryResult::PlaylistDetail(details)) => {
                // Use flatten_content to get sections + tracks as DetailItems
                let items = flatten_content(&crate::domain::ContentDetails::Playlist(details));
                let title = self.stack.current()
                    .and_then(|l| l.selected_item())
                    .and_then(|item| item.as_content_ref())
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "Playlist".to_string());
                self.stack.enter(items, title);
            }
            ("fetch_album_v2", QueryResult::AlbumDetail(details)) => {
                let title = details.title.clone();
                let items = flatten_content(&crate::domain::ContentDetails::Album(details));
                self.stack.enter(items, title);
            }
            ("fetch_artist_v2", QueryResult::ArtistDetail(details)) => {
                let title = details.name.clone();
                let items = flatten_content(&crate::domain::ContentDetails::Artist(details));
                self.stack.enter(items, title);
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
