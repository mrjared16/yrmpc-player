use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use itertools::Itertools;
use modals::{
    add_random_modal::AddRandomModal, decoders::DecodersModal, info_list_modal::InfoListModal,
    input_modal::InputModal, keybinds::KeybindsModal, menu::modal::MenuModal,
    outputs::OutputsModal,
};
use panes::{PaneContainer, Panes, navigator::Navigator, pane_call};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    symbols::border,
    widgets::{Block, Borders},
};
use tab_screen::TabScreen;

use self::{modals::Modal, panes::Pane};
use crate::{
    QueryResult,
    backends::{BackendActions, Capability, Enqueue, GLOBAL_STATUS_UPDATE},
    config::{
        Config,
        cli::{Args, Command},
        keys::{CommonAction, GlobalAction, actions::RateKind},
        tabs::{PaneType, SizedPaneOrSplit, TabName},
        theme::level_styles::LevelStyles,
    },
    core::{
        command::{create_env, run_external},
        config_watcher::ERROR_CONFIG_MODAL_ID,
    },
    ctx::{Ctx, FETCH_SONG_STICKERS, LIKE_STICKER, RATING_STICKER},
    domain::PlaybackState as State,
    mpd::{
        commands::{SeekPosition, idle::IdleEvent},
        errors::{ErrorCode, MpdError, MpdFailureResponse},
        mpd_client::ValueChange,
        version::Version,
    },
    shared::{
        events::{Level, WorkRequest},
        id::Id,
        key_event::KeyEvent,
        macros::{modal, status_error, status_info, status_warn},
        mouse_event::MouseEvent,
        ytdlp::YtDlpHostKind,
    },
    ui::modals::menu::create_rating_modal,
};

pub mod browser;
pub mod dir_or_song;
pub mod dirstack;
pub mod image;
pub mod list_ops;
pub mod modals;
pub mod panes;
pub mod tab_screen;
pub mod views;
pub mod widgets;

#[derive(Debug)]
pub struct StatusMessage {
    pub message: String,
    pub level: Level,
    pub created: std::time::Instant,
    pub timeout: std::time::Duration,
}

#[derive(Debug)]
pub struct Ui<'ui> {
    panes: PaneContainer<'ui>,
    modals: Vec<Box<dyn Modal>>,
    tabs: HashMap<TabName, TabScreen>,
    layout: SizedPaneOrSplit,
    area: Rect,
    /// New Navigator system (enabled when legacy_panes.enabled = false)
    navigator: Option<Navigator>,
}

const OPEN_DECODERS_MODAL: &str = "open_decoders_modal";
const OPEN_OUTPUTS_MODAL: &str = "open_outputs_modal";

macro_rules! active_tab_call {
    ($self:ident, $ctx:ident, $fn:ident($($param:expr),+)) => {
        $self.tabs
            .get_mut(&$ctx.active_tab)
            .context(anyhow!("Expected tab '{}' to be defined. Please report this along with your config.", $ctx.active_tab))?
            .$fn(&mut $self.panes, $($param),+)
    }
}

impl<'ui> Ui<'ui> {
    pub fn new(ctx: &Ctx) -> Result<Ui<'ui>> {
        // Initialize Navigator when new architecture is enabled (legacy disabled)
        let navigator =
            if !ctx.config.legacy_panes.enabled { Some(Navigator::new(ctx)) } else { None };

        Ok(Self {
            panes: PaneContainer::new(ctx)?,
            layout: ctx.config.theme.layout.clone(),
            modals: Vec::default(),
            area: Rect::default(),
            tabs: Self::init_tabs(ctx)?,
            navigator,
        })
    }

    fn init_tabs(ctx: &Ctx) -> Result<HashMap<TabName, TabScreen>> {
        ctx.config
            .tabs
            .tabs
            .iter()
            .map(|(name, screen)| -> Result<_> {
                Ok((name.clone(), TabScreen::new(screen.panes.clone())?))
            })
            .try_collect()
    }

    fn calc_areas(&mut self, area: Rect, _ctx: &Ctx) {
        self.area = area;
    }

    pub fn change_tab(&mut self, new_tab: TabName, ctx: &mut Ctx) -> Result<()> {
        self.layout.for_each_pane(self.area, &mut |pane, _, _, _| {
            match self.panes.get_mut(&pane.pane, ctx)? {
                Panes::TabContent => {
                    active_tab_call!(self, ctx, on_hide(ctx))?;
                }
                _ => {}
            }
            Ok(())
        })?;

        ctx.active_tab = new_tab.clone();
        self.on_event(UiEvent::TabChanged(new_tab), ctx)?;

        self.layout.for_each_pane(self.area, &mut |pane, pane_area, _, _| {
            match self.panes.get_mut(&pane.pane, ctx)? {
                Panes::TabContent => {
                    active_tab_call!(self, ctx, before_show(pane_area, ctx))?;
                }
                _ => {}
            }
            Ok(())
        })
    }

    pub fn render(&mut self, frame: &mut Frame, ctx: &mut Ctx) -> Result<()> {
        let full_area = frame.area();
        if let Some(bg_color) = ctx.config.theme.background_color {
            frame.render_widget(Block::default().style(Style::default().bg(bg_color)), full_area);
        }

        // Calculate layout: if queue panel visible, split area BEFORE rendering panes
        let (main_area, panel_area) = if ctx.queue_panel_visible && self.modals.is_empty() {
            use ratatui::layout::{Constraint, Layout};

            let total_width = full_area.width;
            let panel_percent: u16 = match total_width {
                0..=79 => 0,   // Too narrow, don't show
                80..=99 => 40, // Narrow: 40%
                100..=119 => 35,
                120..=159 => 30,
                _ => 25, // Wide: 25%
            };

            if panel_percent > 0 {
                let areas = Layout::horizontal([
                    Constraint::Percentage(100 - panel_percent),
                    Constraint::Percentage(panel_percent),
                ])
                .areas::<2>(full_area);
                (areas[0], Some(areas[1]))
            } else {
                (full_area, None)
            }
        } else {
            (full_area, None)
        };

        // Use main_area for panes (queue panel takes remaining space)
        self.area = main_area;

        self.layout.for_each_pane_custom_data(
            self.area,
            &mut *frame,
            &mut |pane, pane_area, block, block_area, frame| {
                match self.panes.get_mut(&pane.pane, ctx)? {
                    Panes::TabContent => {
                        // Route through Navigator when enabled
                        if let Some(ref mut navigator) = self.navigator {
                            navigator.render(frame, pane_area, ctx)?;
                        } else {
                            // Legacy path
                            active_tab_call!(self, ctx, render(frame, pane_area, ctx))?;
                        }
                    }
                    mut pane_instance => {
                        pane_call!(pane_instance, render(frame, pane_area, ctx))?;
                    }
                }
                frame.render_widget(block.border_style(ctx.config.as_border_style()), block_area);
                Ok(())
            },
            &mut |block, block_area, frame| {
                frame.render_widget(block.border_style(ctx.config.as_border_style()), block_area);
                Ok(())
            },
        )?;

        // Render queue panel in remaining area (not overlay)
        if let Some(panel_area) = panel_area {
            use crate::ui::widgets::queue_panel::QueuePanel;
            let panel = QueuePanel::new(ctx);
            panel.render(frame, panel_area);
        }

        if ctx.config.theme.modal_backdrop && !self.modals.is_empty() {
            let buffer = frame.buffer_mut();
            buffer.set_style(*buffer.area(), Style::default().fg(Color::DarkGray));
        }

        for modal in &mut self.modals {
            modal.render(frame, ctx)?;
        }

        self.debug_log_ui(ctx);

        Ok(())
    }

    fn debug_log_ui(&mut self, ctx: &mut Ctx) {
        if let Some(path) = &ctx.debug_ui_log {
            let mut state = serde_json::Map::new();
            state.insert(
                "active_tab".to_string(),
                serde_json::json!(format!("{:?}", ctx.active_tab)),
            );

            if let Ok(Panes::Search(p)) = self.panes.get_mut(&PaneType::Search, ctx) {
                state.insert("search".to_string(), p.debug_dump());
            }

            if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let mut writer = std::io::BufWriter::new(file);
                use std::io::Write;
                let _ = writeln!(writer, "{}", serde_json::to_string(&state).unwrap_or_default());
            }
        }
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent, ctx: &mut Ctx) -> Result<()> {
        if let Some(ref mut modal) = self.modals.last_mut() {
            modal.handle_mouse_event(event, ctx)?;
            return Ok(());
        }

        self.layout.for_each_pane(self.area, &mut |pane, _, _, _| {
            match self.panes.get_mut(&pane.pane, ctx)? {
                Panes::TabContent => {
                    active_tab_call!(self, ctx, handle_mouse_event(event, ctx))?;
                }
                mut pane_instance => {
                    pane_call!(pane_instance, handle_mouse_event(event, ctx))?;
                }
            }
            Ok(())
        })
    }

    pub fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<KeyHandleResult> {
        if let Some(ref mut modal) = self.modals.last_mut() {
            modal.handle_key(key, ctx)?;
            return Ok(KeyHandleResult::None);
        }

        // Route through Navigator when enabled (new architecture)
        if let Some(ref mut navigator) = self.navigator {
            navigator.handle_key(key, ctx)?;
        } else {
            active_tab_call!(self, ctx, handle_action(key, ctx))?;
        }

        if let Some(action) = key.as_global_action(ctx) {
            match action {
                GlobalAction::Partition { name: Some(name), autocreate } => {
                    let name = name.clone();
                    let autocreate = *autocreate;
                    ctx.command(move |client| {
                        match client.switch_to_partition(&name) {
                            Ok(()) => {}
                            Err(e) if autocreate => match e.downcast_ref::<MpdError>() {
                                Some(MpdError::Mpd(MpdFailureResponse {
                                    code: ErrorCode::NoExist,
                                    ..
                                })) => {
                                    client.new_partition(&name)?;
                                    client.switch_to_partition(&name)?;
                                }
                                _ => return Err(e),
                            },
                            err @ Err(_) => err?,
                        }
                        Ok(())
                    });
                }
                GlobalAction::Partition { name: None, .. } => {
                    let result = ctx.query_sync(move |client| {
                        let partitions = client.list_partitions()?;
                        Ok(partitions)
                    })?;
                    let modal = MenuModal::new(ctx)
                        .width(60)
                        .list_section(ctx, |section| {
                            if ctx.status.partition == "default" {
                                None
                            } else {
                                let section = section.item("Switch to default partition", |ctx| {
                                    ctx.command(move |client| {
                                        client.switch_to_partition("default")?;
                                        Ok(())
                                    });
                                    Ok(())
                                });

                                Some(section)
                            }
                        })
                        .multi_section(ctx, |section| {
                            let mut section = section
                                .add_action("Switch", |ctx, label| {
                                    ctx.command(move |client| {
                                        client.switch_to_partition(&label)?;
                                        Ok(())
                                    });
                                })
                                .add_action("Delete", |ctx, label| {
                                    ctx.command(move |client| {
                                        client.delete_partition(&label)?;
                                        Ok(())
                                    });
                                });
                            let mut any_non_default = false;
                            for partition in result
                                .iter()
                                .filter(|p| *p != "default" && **p != ctx.status.partition)
                            {
                                section = section.add_item(partition);
                                any_non_default = true;
                            }

                            if any_non_default { Some(section) } else { None }
                        })
                        .input_section(ctx, "New partition:", |section| {
                            let section = section.action(|ctx, value| {
                                if !value.is_empty() {
                                    ctx.command(move |client| {
                                        client.send_start_cmd_list()?;
                                        client.send_new_partition(&value)?;
                                        client.send_switch_to_partition(&value)?;
                                        client.send_execute_cmd_list()?;
                                        client.read_ok()?;
                                        Ok(())
                                    });
                                }
                            });
                            Some(section)
                        })
                        .list_section(ctx, |section| Some(section.item("Cancel", |_ctx| Ok(()))))
                        .build();

                    modal!(ctx, modal);
                }
                GlobalAction::Command { command, .. } => {
                    let cmd = command.parse();
                    log::debug!("executing {cmd:?}");

                    if let Ok(Args { command: Some(cmd), .. }) = cmd
                        && ctx.work_sender.send(WorkRequest::Command(cmd)).is_err()
                    {
                        log::error!("Failed to send command");
                    }
                }
                GlobalAction::CommandMode => {
                    modal!(
                        ctx,
                        InputModal::new(ctx).title("Execute a command").on_confirm(|ctx, value| {
                            match Args::parse_cli_line(value) {
                                Ok(Args {
                                    command:
                                        Some(Command::SearchYt {
                                            query,
                                            provider,
                                            interactive,
                                            limit,
                                            position,
                                        }),
                                    ..
                                }) => {
                                    let kind: YtDlpHostKind = provider.into();

                                    if let Err(e) = ctx.work_sender.send(WorkRequest::SearchYt {
                                        query,
                                        kind,
                                        limit,
                                        interactive,
                                        position,
                                    }) {
                                        log::error!("Failed to send SearchYt work: {e}");
                                    }
                                    Ok(())
                                }

                                Ok(Args { command: Some(cmd), .. }) => {
                                    if ctx.work_sender.send(WorkRequest::Command(cmd)).is_err() {
                                        log::error!("Failed to send command");
                                    }
                                    Ok(())
                                }

                                Ok(_) => {
                                    log::warn!("No subcommand provided");
                                    Ok(())
                                }

                                Err(e) => {
                                    log::error!("Parse error: {e}");
                                    Ok(())
                                }
                            }
                        })
                    );
                }
                GlobalAction::NextTrack if ctx.status.state != State::Stop => {
                    let keep_state = ctx.config.keep_state_on_song_change;
                    let state = ctx.status.state;
                    ctx.command(move |client| {
                        client.next_keep_state(keep_state, state.into())?;
                        Ok(())
                    });
                    // Status update handled by continuous polling for YouTube
                    // backend
                }
                GlobalAction::PreviousTrack if ctx.status.state != State::Stop => {
                    let rewind_to_start = ctx.config.rewind_to_start_sec;
                    let elapsed_sec = ctx.status.elapsed.unwrap_or_default().as_secs();
                    let keep_state = ctx.config.keep_state_on_song_change;
                    let state = ctx.status.state;
                    ctx.command(move |client| {
                        match rewind_to_start {
                            Some(value) => {
                                if elapsed_sec >= value {
                                    client.seek_current(SeekPosition::Absolute(0.0))?;
                                } else {
                                    client.prev_keep_state(keep_state, state.into())?;
                                }
                            }
                            None => {
                                client.prev_keep_state(keep_state, state.into())?;
                            }
                        }
                        Ok(())
                    });
                    // Status update handled by continuous polling for YouTube
                    // backend
                }
                GlobalAction::Stop if matches!(ctx.status.state, State::Play | State::Pause) => {
                    ctx.command(move |client| {
                        client.playback().stop()?;
                        Ok(())
                    });
                    // Status update handled by continuous polling for YouTube
                    // backend
                }
                GlobalAction::ToggleRepeat => {
                    let repeat = !ctx.status.repeat;
                    // Optimistic UI update: immediately reflect the change
                    ctx.status.repeat = repeat;
                    ctx.command(move |client| {
                        client.repeat(repeat)?;
                        Ok(())
                    });
                    // Force immediate render for responsive UI
                    let _ = ctx.render();
                }
                GlobalAction::ToggleRandom => {
                    let random = !ctx.status.random;
                    ctx.status.random = random;
                    ctx.command(move |client| {
                        client.random(random)?;
                        Ok(())
                    });
                    let _ = ctx.render();
                }
                GlobalAction::ToggleSingle => {
                    let single = ctx.status.single;
                    let new_single =
                        if matches!(ctx.config.backend, crate::config::PlayerBackend::YouTube) {
                            single.cycle_skip_oneshot()
                        } else {
                            single.cycle()
                        };
                    ctx.status.single = new_single;
                    ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(
                        move |client| {
                            client.single(new_single.into())?;
                            Ok(QueryResult::Status {
                                data: client.get_status()?,
                                source_event: None,
                            })
                        },
                    );
                    let _ = ctx.render();
                }
                GlobalAction::ToggleConsume => {
                    let consume = ctx.status.consume;
                    let new_consume =
                        if matches!(ctx.config.backend, crate::config::PlayerBackend::YouTube) {
                            consume.cycle_skip_oneshot()
                        } else {
                            consume.cycle()
                        };
                    ctx.status.consume = new_consume;
                    ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(
                        move |client| {
                            client.consume(new_consume.into())?;
                            Ok(QueryResult::Status {
                                data: client.get_status()?,
                                source_event: None,
                            })
                        },
                    );
                    let _ = ctx.render();
                }
                GlobalAction::ToggleSingleOnOff => {
                    let single = ctx.status.single;
                    let new_single = single.cycle_skip_oneshot();
                    ctx.status.single = new_single;
                    ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(
                        move |client| {
                            client.single(new_single.into())?;
                            Ok(QueryResult::Status {
                                data: client.get_status()?,
                                source_event: None,
                            })
                        },
                    );
                    let _ = ctx.render();
                }
                GlobalAction::ToggleConsumeOnOff => {
                    let consume = ctx.status.consume;
                    let new_consume = consume.cycle_skip_oneshot();
                    ctx.status.consume = new_consume;
                    ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(
                        move |client| {
                            client.consume(new_consume.into())?;
                            Ok(QueryResult::Status {
                                data: client.get_status()?,
                                source_event: None,
                            })
                        },
                    );
                    let _ = ctx.render();
                }
                GlobalAction::TogglePause => {
                    if matches!(ctx.status.state, State::Play | State::Pause) {
                        ctx.command(move |client| {
                            client.pause_toggle()?;
                            Ok(())
                        });
                    } else {
                        ctx.command(move |client| {
                            client.playback().play()?;
                            Ok(())
                        });
                    }
                    // Status update handled by continuous polling for YouTube
                    // backend
                }
                GlobalAction::VolumeUp => {
                    let step = ctx.config.volume_step;
                    ctx.command(move |client| {
                        client.volume(ValueChange::Increase(step.into()))?;
                        Ok(())
                    });
                }
                GlobalAction::VolumeDown => {
                    let step = ctx.config.volume_step;
                    ctx.command(move |client| {
                        client.volume(ValueChange::Decrease(step.into()))?;
                        Ok(())
                    });
                }
                GlobalAction::CrossfadeUp => {
                    let current_xfade = ctx.status.xfade.unwrap_or(0);
                    let new_xfade = current_xfade.saturating_add(1);
                    ctx.command(move |client| {
                        client.crossfade(new_xfade)?;
                        Ok(())
                    });
                }
                GlobalAction::CrossfadeDown => {
                    let current_xfade = ctx.status.xfade.unwrap_or(0);
                    let new_xfade = current_xfade.saturating_sub(1);
                    ctx.command(move |client| {
                        client.crossfade(new_xfade)?;
                        Ok(())
                    });
                }
                GlobalAction::SeekForward
                    if matches!(ctx.status.state, State::Play | State::Pause) =>
                {
                    ctx.command(move |client| {
                        client.seek_current(SeekPosition::Relative(5.0))?;
                        Ok(())
                    });
                }
                GlobalAction::SeekBack
                    if matches!(ctx.status.state, State::Play | State::Pause) =>
                {
                    ctx.command(move |client| {
                        client.seek_current(SeekPosition::Relative(-5.0))?;
                        Ok(())
                    });
                }
                GlobalAction::SeekToStart
                    if matches!(ctx.status.state, State::Play | State::Pause) =>
                {
                    ctx.command(move |client| {
                        client.seek_current(SeekPosition::Absolute(0.0))?;
                        Ok(())
                    });
                }
                GlobalAction::Update => {
                    if ctx.supports(Capability::MpdDatabase) {
                        ctx.command(move |client| {
                            client.update(None)?;
                            Ok(())
                        });
                    } else {
                        status_warn!("Database update not supported by this backend");
                    }
                }
                GlobalAction::Rescan => {
                    if ctx.supports(Capability::MpdDatabase) {
                        ctx.command(move |client| {
                            client.rescan(None)?;
                            Ok(())
                        });
                    } else {
                        status_warn!("Database rescan not supported by this backend");
                    }
                }
                GlobalAction::NextTab => {
                    self.change_tab(ctx.config.next_screen(&ctx.active_tab), ctx)?;
                    ctx.render()?;
                }
                GlobalAction::PreviousTab => {
                    self.change_tab(ctx.config.prev_screen(&ctx.active_tab), ctx)?;
                    ctx.render()?;
                }
                GlobalAction::SwitchToTab(name) => {
                    if ctx.config.tabs.names.contains(name) {
                        self.change_tab(name.clone(), ctx)?;
                        ctx.render()?;
                    } else {
                        status_error!(
                            "Tab with name '{}' does not exist. Check your configuration.",
                            name
                        );
                    }
                }
                GlobalAction::NextTrack => {}
                GlobalAction::PreviousTrack => {}
                GlobalAction::Stop => {}
                GlobalAction::SeekBack => {}
                GlobalAction::SeekForward => {}
                GlobalAction::SeekToStart => {}
                GlobalAction::ExternalCommand { command, .. } => {
                    run_external(command.clone(), create_env(ctx, std::iter::empty::<&str>()));
                }
                GlobalAction::Quit => return Ok(KeyHandleResult::Quit),
                GlobalAction::ShowHelp => {
                    let modal = KeybindsModal::new(ctx);
                    modal!(ctx, modal);
                }
                GlobalAction::ShowOutputs => {
                    if ctx.supports(Capability::MpdOutputs) {
                        let current_partition = ctx.status.partition.clone();
                        ctx.query().id(OPEN_OUTPUTS_MODAL).replace_id(OPEN_OUTPUTS_MODAL).query(
                            move |client| {
                                let outputs =
                                    client.list_partitioned_outputs(&current_partition)?;
                                Ok(QueryResult::Outputs(outputs))
                            },
                        );
                    } else {
                        status_warn!("Audio output control not supported by this backend");
                    }
                }
                GlobalAction::ShowDecoders => {
                    if ctx.is_mpd() {
                        ctx.query()
                            .id(OPEN_DECODERS_MODAL)
                            .replace_id(OPEN_DECODERS_MODAL)
                            .query(|client| Ok(QueryResult::Decoders(client.decoders()?)));
                    } else {
                        status_warn!("Decoder info not available for this backend");
                    }
                }
                GlobalAction::ShowCurrentSongInfo => {
                    if let Some((_, current_song)) = ctx.find_current_song_in_queue() {
                        modal!(
                            ctx,
                            InfoListModal::builder()
                                .items(&current_song)
                                .title("Song info")
                                .column_widths(&[30, 70])
                                .build()
                        );
                    } else {
                        status_info!("No song is currently playing");
                    }
                }
                GlobalAction::AddRandom => {
                    modal!(ctx, AddRandomModal::new(ctx));
                }
                GlobalAction::ToggleQueuePanel => {
                    use crate::ui::modals::queue_modal::QueueModal;
                    // Push queue modal (modal handles its own close on Q)
                    modal!(ctx, QueueModal::new());
                }
                GlobalAction::ExpandQueueToTab => {
                    // Save current tab for back navigation
                    ctx.previous_tab = Some(ctx.active_tab.clone());
                    // Hide panel since we're going to full view
                    ctx.queue_panel_visible = false;
                    // Switch to Queue tab
                    self.change_tab("Queue".into(), ctx)?;
                    ctx.render()?;
                }
                GlobalAction::GoBack => {
                    // Restore previous tab if available
                    if let Some(prev_tab) = ctx.previous_tab.take() {
                        self.change_tab(prev_tab, ctx)?;
                        ctx.render()?;
                    }
                }
            }
        } else if let Some(action) = key.as_common_action(ctx) {
            #[allow(
                clippy::collapsible_match,
                reason = "Future expansion, remove when adding other actions"
            )]
            match action {
                CommonAction::Rate { kind, current: true, min_rating, max_rating } => {
                    if !ctx.supports(Capability::MpdStickers) {
                        status_warn!("Rating/stickers not supported by this backend");
                    } else if let Some((_, song)) = ctx.find_current_song_in_queue() {
                        match kind {
                            RateKind::Modal { values, custom, like } => {
                                let items = vec![Enqueue::File { path: song.uri.clone() }];
                                modal!(
                                    ctx,
                                    create_rating_modal(
                                        items,
                                        values.as_slice(),
                                        *min_rating,
                                        *max_rating,
                                        *custom,
                                        *like,
                                        ctx
                                    )
                                );
                            }
                            RateKind::Value(value) => {
                                let uri = song.uri.clone();
                                let value = value.to_string();
                                ctx.command(move |client| {
                                    client.set_sticker(&uri, RATING_STICKER, &value)?;
                                    Ok(())
                                });
                            }
                            RateKind::Like() => {
                                let uri = song.uri.clone();
                                ctx.command(move |client| {
                                    client.set_sticker(&uri, LIKE_STICKER, "2")?;
                                    Ok(())
                                });
                            }
                            RateKind::Dislike() => {
                                let uri = song.uri.clone();
                                ctx.command(move |client| {
                                    client.set_sticker(&uri, LIKE_STICKER, "0")?;
                                    Ok(())
                                });
                            }
                            RateKind::Neutral() => {
                                let uri = song.uri.clone();
                                ctx.command(move |client| {
                                    client.set_sticker(&uri, LIKE_STICKER, "1")?;
                                    Ok(())
                                });
                            }
                        }
                    } else {
                        status_error!("No song is currently playing");
                    }
                }
                _ => {}
            }
        }

        if let crossterm::event::KeyCode::Char(c) = key.code() {
            if let Some(idx) = c.to_digit(10) {
                let idx = idx as usize;
                if idx >= 1 && idx <= 9 {
                    if let Some(tab_name) = ctx.config.tabs.names.get(idx - 1) {
                        self.change_tab(tab_name.clone(), ctx)?;
                        key.stop_propagation();
                        ctx.render()?;
                    }
                }
            }
        }

        Ok(KeyHandleResult::None)
    }

    pub fn before_show(&mut self, area: Rect, ctx: &mut Ctx) -> Result<()> {
        self.calc_areas(area, ctx);

        self.layout.for_each_pane(self.area, &mut |pane, pane_area, _, _| {
            match self.panes.get_mut(&pane.pane, ctx)? {
                Panes::TabContent => {
                    active_tab_call!(self, ctx, before_show(pane_area, ctx))?;
                }
                mut pane_instance => {
                    pane_call!(pane_instance, calculate_areas(pane_area, ctx))?;
                    pane_call!(pane_instance, before_show(ctx))?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn on_ui_app_event(&mut self, event: UiAppEvent, ctx: &mut Ctx) -> Result<()> {
        match event {
            UiAppEvent::Modal(modal) => {
                let existing_modal = modal.replacement_id().and_then(|id| {
                    self.modals
                        .iter_mut()
                        .find(|m| m.replacement_id().as_ref().is_some_and(|m_id| *m_id == id))
                });

                if let Some(existing_modal) = existing_modal {
                    *existing_modal = modal;
                } else {
                    self.modals.push(modal);
                }

                self.on_event(UiEvent::ModalOpened, ctx)?;
                ctx.render()?;
            }
            UiAppEvent::PopConfigErrorModal => {
                let original_len = self.modals.len();
                self.modals
                    .retain(|m| m.replacement_id().is_none_or(|id| id != ERROR_CONFIG_MODAL_ID));
                let new_len = self.modals.len();
                if new_len < original_len {
                    self.on_event(UiEvent::ModalClosed, ctx)?;
                    ctx.render()?;
                }
            }
            UiAppEvent::PopModal(id) => {
                let original_len = self.modals.len();
                self.modals.retain(|m| m.id() != id);
                let new_len = self.modals.len();
                if new_len == 0 {
                    self.on_event(UiEvent::ModalClosed, ctx)?;
                }
                if original_len != new_len {
                    ctx.render()?;
                }
            }
            UiAppEvent::ChangeTab(tab_name) => {
                self.change_tab(tab_name, ctx)?;
                ctx.render()?;
            }
            UiAppEvent::Redraw => {
                ctx.render()?;
            }
            UiAppEvent::OpenAlbum(album_id) => {
                // Switch to Albums tab
                // We assume there is a tab named "Albums" or similar where AlbumsPane is
                // located. If not, we might need to find where AlbumsPane is.
                // For now, let's assume "Albums" tab exists.
                // Or we can iterate to find which tab has AlbumsPane.

                let albums_tab_name = ctx
                    .config
                    .tabs
                    .tabs
                    .iter()
                    .find(|(_, tab)| tab.panes.panes_iter().any(|p| p.pane == PaneType::Albums))
                    .map(|(name, _)| name.clone());

                if let Some(tab_name) = albums_tab_name {
                    self.change_tab(tab_name, ctx)?;

                    // Now we need to tell AlbumsPane to open this album.
                    // We can use `get_mut` to access AlbumsPane.
                    if let Ok(Panes::Albums(albums_pane)) =
                        self.panes.get_mut(&PaneType::Albums, ctx)
                    {
                        // We need to push the album ID to the stack.
                        // The stack path is Vec<String>.
                        // We want to push the album ID.
                        // But we also need to trigger a fetch.

                        // We can manually insert the path and trigger fetch.
                        use crate::ui::dirstack::Path;
                        let _path = Path::from(album_id.clone());

                        // Clear stack and set root? No, we want to keep history if possible,
                        // but since we are jumping from search, maybe just set it as current?
                        // AlbumsPane usually starts with root (list of albums).
                        // If we push album_id, it will be root -> album_id.

                        // But AlbumsPane stack is initialized with list of albums.
                        // If we just push, it might work if the stack logic allows.

                        // Let's try to just push it.
                        // But we need to make sure the stack is initialized?
                        // AlbumsPane initializes in `before_show`.
                        // We just called `change_tab` which calls `before_show`.

                        // However, `before_show` is async-ish (sends query).
                        // So the stack might not be ready.

                        // But we can force it.
                        // Let's just set the stack to have this album as the next item?
                        // Or we can use `ctx.query` to trigger `FETCH_DATA` with this path.

                        // AlbumsPane::fetch_data uses `self.stack().next_path()`.
                        // So we need to insert the path into the stack first.

                        // Wait, `AlbumsPane` has `stack` field.
                        // We can modify it directly.

                        // But `DirStack` doesn't have a simple "push and go" method that also
                        // fetches. We need to simulate user entering a
                        // directory.

                        // Let's try:
                        // 1. Ensure stack has root (maybe empty root is fine).
                        // 2. Insert the album path.
                        // 3. Trigger fetch.

                        // But we don't know the album name, only ID.
                        // `lsinfo` uses ID.
                        // `AlbumsPane` uses `Tag::Album` filter.

                        // If we push "album:ID" as path.
                        // `AlbumsPane::fetch_data` will use it.

                        // Let's see `AlbumsPane::fetch_data`:
                        // let current = selected.as_path().to_owned();
                        // client.find(&[Filter::new(Tag::Album, current)], None)?

                        // So if we push "album:ID", it will search for Tag::Album = "album:ID".
                        // The YouTube backend's find method handles this!

                        albums_pane.stack_mut().push(album_id.clone());

                        // Now trigger fetch.
                        // We can call `fetch_data` manually?
                        // `AlbumsPane` implements `BrowserPane`.
                        // `BrowserPane` has `fetch_data`.
                        // But `fetch_data` takes `selected` item.

                        // We can construct a fake `DirOrSong` representing the album.
                        use crate::ui::dir_or_song::DirOrSong;
                        let fake_item = DirOrSong::Dir {
                            name: album_id.clone(),
                            full_path: album_id.clone(),
                            playlist: false,
                            last_modified: chrono::Utc::now(),
                        };

                        use crate::ui::browser::BrowserPane;
                        albums_pane.fetch_data(&fake_item, ctx)?;
                    }
                }

                ctx.render()?;
            }

            UiAppEvent::OpenArtist(artist_id) => {
                let artists_tab_name = ctx
                    .config
                    .tabs
                    .tabs
                    .iter()
                    .find(|(_, tab)| tab.panes.panes_iter().any(|p| p.pane == PaneType::Artists))
                    .map(|(name, _)| name.clone());

                if let Some(tab_name) = artists_tab_name {
                    self.change_tab(tab_name, ctx)?;

                    if let Ok(Panes::Artists(artist_pane)) =
                        self.panes.get_mut(&PaneType::Artists, ctx)
                    {
                        use crate::ui::dir_or_song::DirOrSong;
                        // Construct a fake DirOrSong to trigger fetch
                        // The name must start with "artist:" for our custom logic in ArtistPane
                        let fake_item = DirOrSong::Dir {
                            name: artist_id.clone(),
                            full_path: artist_id.clone(),
                            playlist: false,
                            last_modified: chrono::Utc::now(),
                        };

                        // We push the path first?
                        // ArtistPane::fetch_data expects the item to be selected or passed.
                        // But we want to navigate TO it.
                        // If we just push to stack, we need to make sure it's valid.

                        // ArtistPane::fetch_data logic:
                        // if current_path.is_empty() && name.starts_with("artist:") -> fetch lsinfo

                        // So we need to be at root (which we are if we just switched tab, usually).
                        // But if we were deep in another artist, we should probably reset?
                        // Or just push?

                        // If we are at root, we can call fetch_data directly with the fake item.
                        // But we need to make sure we are at root.
                        // artist_pane.stack_mut().clear(); // Maybe?

                        // Let's assume we want to reset to root and then open the artist.
                        // Or maybe we want to keep history?
                        // If we are at root, we just call fetch_data.

                        // If we are NOT at root, we might want to go back to root?
                        // Or just push?

                        // Our ArtistPane::fetch_data handles "artist:" ONLY if current_path is
                        // empty. So we MUST be at root.

                        // So let's clear the stack first?
                        // artist_pane.stack_mut().clear(); // No clear method?
                        // stack is DirStack. It has `pop` but maybe not clear.
                        // We can set it to new?

                        // Actually, let's just use `fetch_data` and hope it works or modify
                        // `ArtistPane` to handle it. But `ArtistPane` logic
                        // I wrote: if current_path.is_empty() { ... }

                        // So I should ensure we are at root.
                        // But `DirStack` doesn't expose `clear`.
                        // I can pop until empty?

                        // Let's just assume we are at root or the user wants to go to root.
                        // But wait, `OpenArtist` implies "Go to this artist".

                        // I'll add a `reset` method to `ArtistPane` or `DirStack` later if needed.
                        // For now, I'll try to use `fetch_data` and if it fails because not at
                        // root, I'll fix it. Actually, I can just manually
                        // set the stack if I had access.

                        // Let's just call fetch_data.
                        use crate::ui::browser::BrowserPane;
                        artist_pane.fetch_data(&fake_item, ctx)?;
                    }
                }
                ctx.render()?;
            }
            UiAppEvent::OpenPlaylist(playlist_id) => {
                let playlists_tab_name = ctx
                    .config
                    .tabs
                    .tabs
                    .iter()
                    .find(|(_, tab)| tab.panes.panes_iter().any(|p| p.pane == PaneType::Playlists))
                    .map(|(name, _)| name.clone());

                if let Some(tab_name) = playlists_tab_name {
                    self.change_tab(tab_name, ctx)?;

                    if let Ok(Panes::Playlists(playlist_pane)) =
                        self.panes.get_mut(&PaneType::Playlists, ctx)
                    {
                        use crate::ui::dir_or_song::DirOrSong;
                        let fake_item = DirOrSong::Dir {
                            name: playlist_id.clone(),
                            full_path: playlist_id.clone(),
                            playlist: true,
                            last_modified: chrono::Utc::now(),
                        };

                        use crate::ui::browser::BrowserPane;
                        playlist_pane.fetch_data(&fake_item, ctx)?;
                    }
                }
                ctx.render()?;
            }

            UiAppEvent::NavigateTo { id, kind, title } => {
                use crate::domain::ContentType;

                // 1. Close any open modal
                if !self.modals.is_empty() {
                    self.modals.clear();
                    self.on_event(UiEvent::ModalClosed, ctx)?;
                }

                let title_hint = title.unwrap_or_else(|| match kind {
                    ContentType::Artist => "Artist".to_string(),
                    ContentType::Album => "Album".to_string(),
                    ContentType::Playlist => "Playlist".to_string(),
                    _ => "Content".to_string(),
                });

                if let Some(ref mut navigator) = self.navigator {
                    let entity_type = match kind {
                        ContentType::Artist => panes::navigator_types::DetailId::Artist,
                        ContentType::Album => panes::navigator_types::DetailId::Album,
                        ContentType::Playlist => panes::navigator_types::DetailId::Playlist,
                        _ => {
                            ctx.render()?;
                            return Ok(());
                        }
                    };

                    navigator.request_navigation(
                        panes::navigator_types::EntityRef { entity_type, id, name: title_hint },
                        ctx,
                    )?;
                } else {
                    // Legacy path: switch to Search tab and let SearchPaneV2 own the content stack.
                    let search_tab_name = ctx
                        .config
                        .tabs
                        .tabs
                        .iter()
                        .find(|(_, tab)| tab.panes.panes_iter().any(|p| p.pane == PaneType::Search))
                        .map(|(name, _)| name.clone());

                    if let Some(tab_name) = search_tab_name {
                        self.change_tab(tab_name, ctx)?;

                        if let Ok(Panes::SearchV2(search_pane)) =
                            self.panes.get_mut(&PaneType::Search, ctx)
                        {
                            search_pane.navigate_to(id, kind, title_hint, ctx);
                        }
                    }
                }
                ctx.render()?;
            }
        }
        Ok(())
    }

    pub fn resize(&mut self, area: Rect, ctx: &Ctx) -> Result<()> {
        log::trace!(area:?; "Terminal was resized");
        self.calc_areas(area, ctx);

        self.layout.for_each_pane(self.area, &mut |pane, pane_area, _, _| {
            match self.panes.get_mut(&pane.pane, ctx)? {
                Panes::TabContent => {
                    active_tab_call!(self, ctx, resize(pane_area, ctx))?;
                }
                mut pane_instance => {
                    pane_call!(pane_instance, calculate_areas(pane_area, ctx))?;
                    pane_call!(pane_instance, resize(pane_area, ctx))?;
                }
            }
            Ok(())
        })
    }

    pub fn on_event(&mut self, mut event: UiEvent, ctx: &mut Ctx) -> Result<()> {
        match event {
            UiEvent::Database => {
                status_warn!(
                    "The music database has been updated. Some parts of the UI may have been reinitialized to prevent inconsistent behaviours."
                );
            }
            UiEvent::ConfigChanged => {
                // Call on_hide for all panes in the current tab and current layout because they
                // might not be visible after the change
                self.layout.for_each_pane(self.area, &mut |pane, _, _, _| {
                    match self.panes.get_mut(&pane.pane, ctx)? {
                        Panes::TabContent => {
                            active_tab_call!(self, ctx, on_hide(ctx))?;
                        }
                        mut pane_instance => {
                            pane_call!(pane_instance, on_hide(ctx))?;
                        }
                    }
                    Ok(())
                })?;

                self.layout = ctx.config.theme.layout.clone();
                let new_active_tab = ctx
                    .config
                    .tabs
                    .names
                    .iter()
                    .find(|tab| tab == &&ctx.active_tab)
                    .or(ctx.config.tabs.names.first())
                    .context("Expected at least one tab")?;

                let mut old_other_panes = std::mem::take(&mut self.panes.others);
                for (key, new_other_pane) in PaneContainer::init_other_panes(ctx) {
                    let old = old_other_panes.remove(&key);
                    self.panes.others.insert(key, old.unwrap_or(new_other_pane));
                }
                // We have to be careful about the order of operations here as they might cause
                // a panic if done incorrectly
                self.tabs = Self::init_tabs(ctx)?;
                ctx.active_tab = new_active_tab.clone();
                self.on_event(UiEvent::TabChanged(new_active_tab.clone()), ctx)?;

                // Call before_show here, because we have "hidden" all the panes before and this
                // will force them to reinitialize
                self.before_show(self.area, ctx)?;
            }
            _ => {}
        }

        for pane_type in &ctx.config.active_panes {
            let visible = self
                .tabs
                .get(&ctx.active_tab)
                .is_some_and(|tab| tab.panes.panes_iter().any(|pane| pane.pane == *pane_type))
                || self.layout.panes_iter().any(|pane| pane.pane == *pane_type);

            match self.panes.get_mut(pane_type, ctx)? {
                #[cfg(debug_assertions)]
                Panes::Logs(p) => p.on_event(&mut event, visible, ctx),
                Panes::Queue(p) => p.on_event(&mut event, visible, ctx),
                Panes::QueueV2(p) => p.on_event(&mut event, visible, ctx),
                Panes::Directories(p) => p.on_event(&mut event, visible, ctx),
                Panes::Albums(p) => p.on_event(&mut event, visible, ctx),
                Panes::Artists(p) => p.on_event(&mut event, visible, ctx),
                Panes::Playlists(p) => p.on_event(&mut event, visible, ctx),
                Panes::Search(p) => p.on_event(&mut event, visible, ctx),
                Panes::SearchV2(p) => p.on_event(&mut event, visible, ctx),
                Panes::AlbumArtists(p) => p.on_event(&mut event, visible, ctx),
                Panes::AlbumArt(p) => p.on_event(&mut event, visible, ctx),
                Panes::Lyrics(p) => p.on_event(&mut event, visible, ctx),
                Panes::ProgressBar(p) => p.on_event(&mut event, visible, ctx),
                Panes::Header(p) => p.on_event(&mut event, visible, ctx),
                Panes::Tabs(p) => p.on_event(&mut event, visible, ctx),
                #[cfg(debug_assertions)]
                Panes::FrameCount(p) => p.on_event(&mut event, visible, ctx),
                Panes::Others(p) => p.on_event(&mut event, visible, ctx),
                Panes::Cava(p) => p.on_event(&mut event, visible, ctx),
                Panes::Library(p) => p.on_event(&mut event, visible, ctx),
                // Property and the dummy TabContent pane do not need to receive events
                Panes::Property(_) | Panes::TabContent => Ok(()),
            }?;
        }

        for modal in &mut self.modals {
            modal.on_event(&mut event, ctx)?;
        }

        // Route to Navigator when enabled
        if let Some(ref mut navigator) = self.navigator {
            navigator.on_event(&mut event, ctx)?;
        }

        Ok(())
    }

    pub(crate) fn on_command_finished(
        &mut self,
        id: &'static str,
        pane: Option<PaneType>,
        data: QueryResult,
        ctx: &mut Ctx,
    ) -> Result<()> {
        match pane {
            Some(pane_type) => {
                // Route through Navigator for panes it manages (Search, Queue)
                // when new architecture is enabled
                if let Some(ref mut navigator) = self.navigator {
                    if matches!(pane_type, PaneType::Search | PaneType::Queue) {
                        navigator.on_query_finished(id, data, pane_type, ctx)?;
                        ctx.render()?;
                        return Ok(());
                    }
                }

                let visible =
                    self.tabs.get(&ctx.active_tab).is_some_and(|tab| {
                        tab.panes.panes_iter().any(|pane| pane.pane == pane_type)
                    }) || self.layout.panes_iter().any(|pane| pane.pane == pane_type);

                match self.panes.get_mut(&pane_type, ctx)? {
                    #[cfg(debug_assertions)]
                    Panes::Logs(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Queue(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::QueueV2(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Directories(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Albums(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Artists(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Playlists(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Search(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::SearchV2(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::AlbumArtists(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::AlbumArt(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Lyrics(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::ProgressBar(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Header(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Tabs(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Others(p) => p.on_query_finished(id, data, visible, ctx),
                    #[cfg(debug_assertions)]
                    Panes::FrameCount(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Cava(p) => p.on_query_finished(id, data, visible, ctx),
                    Panes::Library(p) => p.on_query_finished(id, data, visible, ctx),
                    // Property and the dummy TabContent pane do not need to receive command
                    // notifications
                    Panes::Property(_) | Panes::TabContent => Ok(()),
                }?;
                // Auto-render after query results are processed
                ctx.render()?;
            }
            None => match (id, data) {
                (OPEN_OUTPUTS_MODAL, QueryResult::Outputs(outputs)) => {
                    modal!(ctx, OutputsModal::new(outputs));
                }
                (OPEN_DECODERS_MODAL, QueryResult::Decoders(decoders)) => {
                    modal!(ctx, DecodersModal::new(decoders));
                }
                (FETCH_SONG_STICKERS, QueryResult::SongStickers(stickers)) => {
                    for (k, v) in stickers {
                        // Assume all stickers were fetched for each song so simple replace is
                        // enough
                        ctx.set_song_stickers(k, v);
                    }
                    ctx.render()?;
                }
                (id, mut data) => {
                    // TODO a proper modal target
                    for modal in &mut self.modals {
                        modal.on_query_finished(id, &mut data, ctx)?;
                    }
                }
            },
        }

        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum UiAppEvent {
    Modal(Box<dyn Modal + Send + Sync>),
    PopModal(Id),
    PopConfigErrorModal,
    ChangeTab(TabName),
    Redraw,
    // Legacy events - kept for compatibility with MPD panes
    OpenAlbum(String),
    OpenArtist(String),
    OpenPlaylist(String),
    /// Navigate to content from anywhere (queue modal, etc.)
    /// Unified alternative to OpenAlbum/OpenArtist/OpenPlaylist for the new
    /// architecture.
    NavigateTo {
        id: String,
        kind: crate::domain::ContentType,
        /// Optional title hint for breadcrumb (avoids extra fetch if known)
        title: Option<String>,
    },
}

#[derive(Debug, Eq, Hash, PartialEq)]
#[allow(dead_code)]
pub enum UiEvent {
    Player,
    QueueChanged,
    Database,
    Output,
    StoredPlaylist,
    LogAdded(Vec<u8>),
    ModalOpened,
    ModalClosed,
    Exit,
    LyricsIndexed,
    SongChanged,
    Reconnected,
    TabChanged(TabName),
    Displayed,
    Hidden,
    ConfigChanged,
    PlaybackStateChanged,
    Redraw,
}

impl TryFrom<IdleEvent> for UiEvent {
    type Error = ();

    fn try_from(event: IdleEvent) -> Result<Self, ()> {
        Ok(match event {
            IdleEvent::Player => UiEvent::Player,
            IdleEvent::Database => UiEvent::Database,
            IdleEvent::StoredPlaylist => UiEvent::StoredPlaylist,
            IdleEvent::Output => UiEvent::Output,
            _ => return Err(()),
        })
    }
}

pub enum KeyHandleResult {
    None,
    Quit,
}

impl From<&Level> for Color {
    fn from(value: &Level) -> Self {
        match value {
            Level::Info => Color::Blue,
            Level::Warn => Color::Yellow,
            Level::Error => Color::Red,
            Level::Debug => Color::LightGreen,
            Level::Trace => Color::Magenta,
        }
    }
}

impl Level {
    pub fn into_style(self, config: &LevelStyles) -> Style {
        match self {
            Level::Trace => config.trace,
            Level::Debug => config.debug,
            Level::Warn => config.warn,
            Level::Error => config.error,
            Level::Info => config.info,
        }
    }
}

impl Config {
    fn next_screen(&self, current_screen: &TabName) -> TabName {
        let names = &self.tabs.names;
        names
            .iter()
            .enumerate()
            .find(|(_, s)| *s == current_screen)
            .and_then(|(idx, _)| names.get((idx + 1) % names.len()))
            .unwrap_or(current_screen)
            .clone()
    }

    fn prev_screen(&self, current_screen: &TabName) -> TabName {
        let names = &self.tabs.names;
        self.tabs
            .names
            .iter()
            .enumerate()
            .find(|(_, s)| *s == current_screen)
            .and_then(|(idx, _)| {
                names.get((if idx == 0 { names.len() - 1 } else { idx - 1 }) % names.len())
            })
            .unwrap_or(current_screen)
            .clone()
    }

    fn as_header_table_block(&self) -> ratatui::widgets::Block<'_> {
        if !self.theme.draw_borders {
            return ratatui::widgets::Block::default();
        }
        Block::default().border_style(self.as_border_style())
    }

    fn as_tabs_block<'block>(&self) -> ratatui::widgets::Block<'block> {
        if !self.theme.draw_borders {
            return ratatui::widgets::Block::default()/* .padding(Padding::new(0, 0, 1, 1)) */;
        }

        ratatui::widgets::Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_set(border::ONE_EIGHTH_WIDE)
            .border_style(self.as_border_style())
    }

    fn as_border_style(&self) -> ratatui::style::Style {
        self.theme.borders_style
    }

    fn as_focused_border_style(&self) -> ratatui::style::Style {
        self.theme.highlight_border_style
    }

    fn as_text_style(&self) -> ratatui::style::Style {
        self.theme.text_color.map(|color| Style::default().fg(color)).unwrap_or_default()
    }

    fn as_styled_scrollbar(&self) -> Option<ratatui::widgets::Scrollbar<'_>> {
        let scrollbar = self.theme.scrollbar.as_ref()?;
        let symbols = &scrollbar.symbols;
        Some(
            ratatui::widgets::Scrollbar::default()
                .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                .track_symbol(if symbols[0].is_empty() { None } else { Some(&symbols[0]) })
                .thumb_symbol(&scrollbar.symbols[1])
                .begin_symbol(if symbols[2].is_empty() { None } else { Some(&symbols[2]) })
                .end_symbol(if symbols[3].is_empty() { None } else { Some(&symbols[3]) })
                .track_style(scrollbar.track_style)
                .begin_style(scrollbar.ends_style)
                .end_style(scrollbar.ends_style)
                .thumb_style(scrollbar.thumb_style),
        )
    }
}
