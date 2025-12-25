//! Queue Modal - Right-anchored sidebar showing current queue
//!
//! Quick access to queue without leaving current view.

use anyhow::Result;
use ratatui::{
    Frame,
    style::Style,
    widgets::{Block, Clear},
};

use crate::{
    config::keys::{CommonAction, GlobalAction},
    ctx::Ctx,
    domain::ContentType,
    shared::{
        events::AppEvent,
        id::{self, Id},
        key_event::KeyEvent,
        mouse_event::MouseEvent,
    },
    ui::{
        UiAppEvent,
        list_ops::{MoveDirection, QueueListBehavior},
        modals::{Modal, RectExt},
        widgets::interactive_list_view::{InteractiveListView, NavConfig},
    },
};

#[derive(Debug)]
pub struct QueueModal {
    id: Id,
    list_view: InteractiveListView,
}

impl QueueModal {
    pub fn new() -> Self {
        Self {
            id: id::new(),
            list_view: InteractiveListView::new(),
        }
    }

    /// Navigate to artist details for the selected queue item.
    ///
    /// Uses the artist_browse_id if available, otherwise falls back to artist name search.
    fn navigate_to_artist(&self, ctx: &Ctx) {
        let Some(idx) = self.list_view.selected() else {
            return;
        };
        let Some(song) = ctx.queue.get(idx) else {
            return;
        };

        // Try to get artist browse ID from metadata
        // YouTube Music songs have this when added via search
        if let Some(artist_id) = song.metadata.get("artist_browse_id").and_then(|v| v.first()) {
            log::info!("Navigating to artist ID: {}", artist_id);
            let artist_name = song.artist().unwrap_or("Artist").to_string();
            let _ = ctx.app_event_sender.send(AppEvent::UiEvent(UiAppEvent::NavigateTo {
                id: artist_id.clone(),
                kind: ContentType::Artist,
                title: Some(artist_name),
            }));
        } else if let Some(artist_name) = song.artist() {
            // Fallback: use artist name (less reliable, may not find exact match)
            log::warn!("No artist_browse_id for '{}', using name search", artist_name);
            // For now, just log - a future improvement could search by name
            // For YouTube Music, we don't have a reliable artist ID in queue items
            // unless they were added from search results with full metadata
        }
    }
}

// Implement QueueListBehavior trait for shared action logic
impl QueueListBehavior for QueueModal {
    fn list_view(&self) -> &InteractiveListView {
        &self.list_view
    }

    fn list_view_mut(&mut self) -> &mut InteractiveListView {
        &mut self.list_view
    }
    // Uses default implementations for play_selected, delete_selected, move_selected
}


impl Modal for QueueModal {
    fn id(&self) -> Id {
        self.id
    }

    fn render(&mut self, frame: &mut Frame, ctx: &mut Ctx) -> Result<()> {
        // Calculate responsive width
        let total_width = frame.area().width;
        let width_percent: u16 = match total_width {
            0..=79 => 50,    // Narrow: take more space
            80..=119 => 40,
            120..=159 => 35,
            _ => 30,         // Wide: 30%
        };
        
        let area = frame.area().right_anchored(width_percent);
        
        // Clear the modal area (removes content behind)
        frame.render_widget(Clear, area);
        
        // Render solid background
        if let Some(bg_color) = ctx.config.theme.modal_background_color {
            frame.render_widget(Block::default().style(Style::default().bg(bg_color)), area);
        } else {
            frame.render_widget(Block::default().style(ctx.config.as_text_style()), area);
        }
        
        // Find current song for highlight
        let current_song_id = ctx.find_current_song_in_queue().map(|(_, song)| song.id);
        
        // Render using InteractiveListView
        self.list_view.render(
            frame,
            area,
            ctx,
            &ctx.queue,
            Some("Queue"),
            |_idx, song| current_song_id.is_some_and(|id| id == song.id),
        );
        
        Ok(())
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        // Check for global action first
        if let Some(action) = key.as_global_action(ctx) {
            match action {
                GlobalAction::ToggleQueuePanel => {
                    self.hide(ctx)?;
                    key.stop_propagation();
                    return Ok(());
                }
                GlobalAction::ExpandQueueToTab => {
                    ctx.previous_tab = Some(ctx.active_tab.clone());
                    self.hide(ctx)?;
                    return Ok(());
                }
                // Passthrough playback controls
                GlobalAction::TogglePause 
                | GlobalAction::NextTrack 
                | GlobalAction::PreviousTrack
                | GlobalAction::VolumeUp
                | GlobalAction::VolumeDown => {
                    return Ok(());
                }
                _ => {}
            }
        }
        
        // Check for common navigation actions
        if let Some(action) = key.as_common_action(ctx) {
            match action {
                CommonAction::Up => {
                    self.list_view.select_prev(&ctx.queue, NavConfig::default());
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Down => {
                    self.list_view.select_next(&ctx.queue, NavConfig::default());
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Confirm => {
                    QueueListBehavior::play_selected(self, ctx);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Close => {
                    self.hide(ctx)?;
                    key.stop_propagation();
                }
                CommonAction::Delete => {
                    QueueListBehavior::delete_selected(self, ctx);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Top => {
                    self.list_view.select_first(&ctx.queue);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Bottom => {
                    self.list_view.select_last(&ctx.queue);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::MoveUp => {
                    QueueListBehavior::move_selected(self, MoveDirection::Up, ctx);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::MoveDown => {
                    QueueListBehavior::move_selected(self, MoveDirection::Down, ctx);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Right => {
                    // Navigate to artist details
                    self.navigate_to_artist(ctx);
                    key.stop_propagation();
                }
                _ => {}
            }
        }
        
        Ok(())
    }

    fn handle_mouse_event(&mut self, _event: MouseEvent, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }
}
