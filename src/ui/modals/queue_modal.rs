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
        widgets::selectable_list::{NavConfig, SelectableList},
    },
};

#[derive(Debug)]
pub struct QueueModal {
    id: Id,
    list_view: SelectableList,
}

impl QueueModal {
    pub fn new() -> Self {
        Self { id: id::new(), list_view: SelectableList::new() }
    }

    /// Navigate to artist details for the selected queue item.
    ///
    /// Uses the artist_browse_id if available, otherwise falls back to artist
    /// name search.
    fn navigate_to_artist(&self, ctx: &Ctx) {
        let Some(idx) = self.list_view.selected() else {
            return;
        };
        let Some(song) = ctx.queue_store().get(idx) else {
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
            // For YouTube Music, we don't have a reliable artist ID in queue
            // items unless they were added from search results with
            // full metadata
        }
    }
}

// Implement QueueListBehavior trait for shared action logic
impl QueueListBehavior for QueueModal {
    fn list_view(&self) -> &SelectableList {
        &self.list_view
    }

    fn list_view_mut(&mut self) -> &mut SelectableList {
        &mut self.list_view
    }
    // Uses default implementations for play_selected, delete_selected,
    // move_selected
}

impl Modal for QueueModal {
    fn id(&self) -> Id {
        self.id
    }

    fn render(&mut self, frame: &mut Frame, ctx: &mut Ctx) -> Result<()> {
        // Calculate responsive width
        let total_width = frame.area().width;
        let width_percent: u16 = match total_width {
            0..=79 => 50, // Narrow: take more space
            80..=119 => 40,
            120..=159 => 35,
            _ => 30, // Wide: 30%
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

        // Get queue snapshot for rendering
        let queue = ctx.queue_store().read();

        // Render using InteractiveListView
        self.list_view.render(frame, area, ctx, &*queue, Some("Queue"), |_idx, song| {
            current_song_id.is_some_and(|id| id == song.id)
        });

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
                    let queue = ctx.queue_store().read();
                    self.list_view.select_prev(&*queue, NavConfig::default());
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Down => {
                    let queue = ctx.queue_store().read();
                    self.list_view.select_next(&*queue, NavConfig::default());
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
                    let queue = ctx.queue_store().read();
                    self.list_view.select_first(&*queue);
                    key.stop_propagation();
                    ctx.render()?;
                }
                CommonAction::Bottom => {
                    let queue = ctx.queue_store().read();
                    self.list_view.select_last(&*queue);
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

        // Check for queue-specific actions (d key maps to QueueActions::Delete)
        if let Some(action) = key.as_queue_action(ctx) {
            use crate::config::keys::QueueActions;
            match action {
                QueueActions::Delete => {
                    QueueListBehavior::delete_selected(self, ctx);
                    key.stop_propagation();
                    ctx.render()?;
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

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::{HashMap, HashSet},
        sync::{Arc, RwLock},
    };

    use crossbeam::channel::unbounded;
    use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

    use super::*;
    use crate::{
        config::{Config, keys::QueueActions},
        ctx::Ctx,
        domain::{Song, Status},
        mpd::version::Version,
        shared::{image_cache::ImageCache, ring_vec::RingVec},
    };

    fn create_test_ctx() -> Ctx {
        let (tx, _rx) = unbounded();
        let (work_tx, _work_rx) = unbounded();
        let (client_tx, _client_rx) = unbounded();

        // Create config with proper default keybindings
        let key_config_file = crate::config::keys::KeyConfigFile::default();
        let key_config: crate::config::keys::KeyConfig = key_config_file.try_into().unwrap();
        let config = Config::default();
        // Replace empty keybinds with proper defaults
        let config_with_keybinds = Config { keybinds: key_config, ..config };

        Ctx {
            backend_version: Version::new(0, 0, 0),
            config: Arc::new(config_with_keybinds),
            status: Status::default(),
            image_cache: ImageCache::new(tx.clone()),
            app_state: Arc::new(RwLock::new(crate::app_state::AppState::default())),
            controllers: crate::core::controllers::Controllers::new(
                vec![],
                tx.clone(),
                client_tx.clone(),
            ),
            stickers: HashMap::new(),
            active_tab: crate::config::tabs::TabName::from("Queue"),
            supported_commands: HashSet::new(),
            capabilities: &[],
            db_update_start: None,
            app_event_sender: tx.clone(),
            work_sender: work_tx,
            client_request_sender: client_tx.clone(),
            needs_render: Cell::new(false),
            stickers_to_fetch: RefCell::new(HashSet::new()),
            lrc_index: Default::default(),
            rendered_frames: 0,
            messages: RingVec::default(),
            last_status_update: std::time::Instant::now(),
            song_played: None,
            stickers_supported: crate::ctx::StickersSupport::Unsupported,
            scheduler: crate::core::scheduler::Scheduler::new((tx, client_tx)),
            debug_ui_log: None,
            queue_panel_visible: false,
            previous_tab: None,
        }
    }

    /// Test that pressing 'd' key triggers QueueActions::Delete.
    ///
    /// This verifies that handle_key() correctly checks as_queue_action()
    /// and handles the delete action. Note: actual queue modification happens
    /// asynchronously via backend commands, so we verify key handling, not
    /// queue state.
    #[test]
    fn pressing_d_key_should_trigger_delete_action() {
        let mut ctx = create_test_ctx();

        let song = Song { id: Some(1), uri: "test_song_uri".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song]);

        // Create modal and select first item
        let mut modal = QueueModal::new();
        modal.list_view.select(Some(0));

        // VERIFICATION 1: 'd' maps to QueueActions::Delete in keybinds
        let crossterm_key = CKeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let mut key = crate::shared::key_event::KeyEvent::from(crossterm_key);
        let queue_action = key.as_queue_action(&ctx);
        assert_eq!(
            queue_action,
            Some(QueueActions::Delete),
            "The 'd' key should map to QueueActions::Delete"
        );

        // VERIFICATION 2: as_common_action does NOT recognize 'd'
        // (CommonAction::Delete is 'D' Shift+d, not lowercase 'd')
        let crossterm_key = CKeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let mut key = crate::shared::key_event::KeyEvent::from(crossterm_key);
        let common_action = key.as_common_action(&ctx);
        assert!(
            common_action.is_none(),
            "as_common_action should NOT recognize 'd' (that's QueueActions, not CommonAction)"
        );

        // VERIFICATION 3: handle_key processes 'd' and stops propagation
        // (this proves the key was handled, even though queue modification is async)
        // We verify by checking that as_queue_action returns None after handle_key
        // (meaning the key was consumed/handled)
        let crossterm_key = CKeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let mut key = crate::shared::key_event::KeyEvent::from(crossterm_key);
        let _ = modal.handle_key(&mut key, &mut ctx);

        // Try to get queue action again - should be None because key was handled
        let queue_action_after = key.as_queue_action(&ctx);
        assert!(
            queue_action_after.is_none(),
            "Key should be consumed after handle_key processes 'd' as QueueActions::Delete"
        );
    }

    /// Verify Shift+D does work (CommonAction::Delete is handled)
    #[test]
    fn pressing_shift_d_should_trigger_delete_via_common_action() {
        let mut ctx = create_test_ctx();

        let song = Song { id: Some(1), uri: "test_song_uri".to_string(), ..Default::default() };
        ctx.queue_store().reconcile(vec![song]);

        let mut modal = QueueModal::new();
        modal.list_view.select(Some(0));

        // Simulate pressing 'D' (Shift+d - maps to CommonAction::Delete)
        let crossterm_key = CKeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let mut key = crate::shared::key_event::KeyEvent::from(crossterm_key);

        // Verify the mapping
        let common_action = key.as_common_action(&ctx);
        assert_eq!(
            common_action,
            Some(&crate::config::keys::CommonAction::Delete),
            "Shift+D should map to CommonAction::Delete"
        );

        // Reset for actual test
        let crossterm_key = CKeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let mut key = crate::shared::key_event::KeyEvent::from(crossterm_key);
        let _ = modal.handle_key(&mut key, &mut ctx);

        // Shift+D should work - queue modification happens asynchronously via
        // backend. We verify the key was recognized (we can't easily
        // check queue modification without mocking the client command
        // sender)
    }
}
