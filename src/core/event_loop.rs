use std::{
    collections::HashSet,
    ops::Sub,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, RecvTimeoutError};
use ratatui::{Terminal, layout::Rect, prelude::Backend};

use super::command::{create_env, run_external};
use crate::{
    WorkRequest,
    backends::{
        BackendActions, EXTERNAL_COMMAND, GLOBAL_QUEUE_UPDATE, GLOBAL_STATUS_UPDATE,
        GLOBAL_STICKERS_UPDATE, GLOBAL_VOLUME_UPDATE, QueryResult, run_status_update,
    },
    config::{
        Config,
        cli::{Command, RemoteCommandQuery},
    },
    ctx::Ctx,
    domain::PlaybackState as State,
    mpd::{
        commands::{IdleEvent, volume::Volume},
        mpd_client::SaveMode,
    },
    shared::{
        events::{AppEvent, WorkDone},
        ext::error::ErrorExt,
        id::{self, Id},
        macros::{status_error, status_warn},
    },
    ui::{
        KeyHandleResult, StatusMessage, Ui, UiAppEvent, UiEvent,
        modals::{info_modal::InfoModal, select_modal::SelectModal},
    },
};

static ON_RESIZE_SCHEDULE_ID: LazyLock<Id> = LazyLock::new(id::new);

pub(crate) fn init<B: Backend + std::io::Write + Send + 'static>(
    ctx: Ctx,
    event_rx: Receiver<AppEvent>,
    terminal: Terminal<B>,
) -> std::io::Result<std::thread::JoinHandle<Terminal<B>>> {
    std::thread::Builder::new()
        .name("main".to_owned())
        .spawn(move || main_task(ctx, event_rx, terminal))
}

fn main_task<B: Backend + std::io::Write>(
    mut ctx: Ctx,
    event_rx: Receiver<AppEvent>,
    mut terminal: Terminal<B>,
) -> Terminal<B> {
    let size = terminal.size().expect("To be able to get terminal size");
    let area = Rect::new(0, 0, size.width, size.height);
    let mut ui = Ui::new(&ctx).expect("UI to be created correctly");
    let event_receiver = event_rx;
    let mut render_wanted = false;
    let max_fps = f64::from(ctx.config.max_fps);
    let mut min_frame_duration = Duration::from_secs_f64(1f64 / max_fps);
    let mut last_render = std::time::Instant::now().sub(Duration::from_secs(10));
    let mut additional_evs = HashSet::new();
    let mut connected = true;
    ui.before_show(area, &mut ctx).expect("Initial render init to succeed");
    let mut _update_loop_guard = None;
    let mut _update_db_loop_guard = None;
    // Separate render timer for progress bar updates (doesn't poll backend)
    let mut _playback_render_guard = None;

    // Tmux hooks have to be initialized after ui, because ueberzugpp replaces all
    // hooks on its init instead of simply appending and might break rmpc's hooks
    let mut tmux = match crate::shared::tmux::TmuxHooks::new() {
        Ok(Some(val)) => Some(val),
        Ok(None) => None,
        Err(err) => {
            log::error!(error:? = err; "Failed to install tmux hooks");
            None
        }
    };

    // Execute on_song_change at startup if
    // configured and current song is available.
    if ctx.config.exec_on_song_change_at_start
        && let Some((_, _song)) = ctx.find_current_song_in_queue()
        && let Some(command) = &ctx.config.on_song_change
    {
        let env = create_env(&ctx, std::iter::empty());
        run_external(command.clone(), env);
    }

    // For YouTube backend: Start status polling (adaptive based on state)
    // This is the architectural fix - YouTube client has no event system, so we
    // must poll
    let is_youtube = matches!(ctx.config.backend, crate::config::PlayerBackend::YouTube);

    if is_youtube {
        // YouTube backend: Start polling - interval depends on playback state
        // Playing: poll frequently for timer updates (default 1000ms)
        // Paused/Stopped: poll less frequently just for state detection (5000ms)
        let interval = if ctx.status.state == State::Play {
            ctx.config.status_update_interval_ms.unwrap_or(1000)
        } else {
            5000 // 5 seconds when not playing - just to detect state changes
        };
        _update_loop_guard =
            Some(ctx.scheduler.repeated(Duration::from_millis(interval), run_status_update));

        // If already playing at startup, also start the render timer for progress bar
        if ctx.status.state == State::Play {
            _playback_render_guard =
                Some(ctx.scheduler.repeated(Duration::from_secs(1), |(tx, _)| {
                    tx.send(AppEvent::RequestRender)?;
                    Ok(())
                }));
        }

        ctx.song_played = ctx.status.elapsed;
        log::info!("YouTube backend: started adaptive status polling ({}ms)", interval);
    } else {
        // MPD backend: Only poll when playing (idle events handle state changes)
        match ctx.status.state {
            State::Play => {
                // Start update loop since a song is playing on startup
                _update_loop_guard = ctx
                    .config
                    .status_update_interval_ms
                    .map(Duration::from_millis)
                    .map(|interval| ctx.scheduler.repeated(interval, run_status_update));

                // Also start render timer for progress bar
                _playback_render_guard =
                    Some(ctx.scheduler.repeated(Duration::from_secs(1), |(tx, _)| {
                        tx.send(AppEvent::RequestRender)?;
                        Ok(())
                    }));

                ctx.song_played = ctx.status.elapsed;
            }
            State::Pause => {
                ctx.song_played = ctx.status.elapsed;
            }
            State::Stop => {}
        }
    }

    loop {
        let now = std::time::Instant::now();

        let event = if render_wanted {
            // Calculate time until next frame
            let timeout =
                min_frame_duration.checked_sub(now - last_render).unwrap_or(Duration::ZERO);
            // Ensure minimum 1ms sleep to prevent busy-waiting and reduce CPU usage
            let timeout = timeout.max(Duration::from_millis(1));
            match event_receiver.recv_timeout(timeout) {
                Ok(v) => Some(v),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => None,
            }
        } else {
            event_receiver.recv().ok()
        };

        if let Some(event) = event {
            match event {
                AppEvent::ConfigChanged { config: mut new_config, keep_old_theme } => {
                    // Technical limitation. Keep the old image backend because it was not rechecked
                    // anyway. Sending the escape sequences to determine image support would mess up
                    // the terminal output at this point.
                    new_config.album_art.method = ctx.config.album_art.method;
                    if keep_old_theme {
                        new_config.theme = ctx.config.theme.clone();
                    }

                    if let Err(err) = new_config.validate() {
                        status_error!(error:? = err; "Cannot change config, invalid value: '{err}'");
                        continue;
                    }

                    new_config.active_panes =
                        Config::calc_active_panes(&new_config.tabs.tabs, &new_config.theme.layout);
                    ctx.config = Arc::new(*new_config);
                    let max_fps = f64::from(ctx.config.max_fps);
                    min_frame_duration = Duration::from_secs_f64(1f64 / max_fps);

                    if let Err(err) = ui.on_event(UiEvent::ConfigChanged, &mut ctx) {
                        log::error!(error:? = err; "UI failed to handle config changed event");
                        continue;
                    }

                    // Need to clear the terminal to avoid artifacts from album art and other
                    // elements
                    if let Err(err) = terminal.clear() {
                        log::error!(error:? = err; "Failed to clear terminal after config change");
                        continue;
                    }

                    render_wanted = true;
                }
                AppEvent::ThemeChanged { theme } => {
                    let mut config = ctx.config.as_ref().clone();
                    config.theme = *theme;
                    if let Err(err) = config.validate() {
                        status_error!(error:? = err; "Cannot change theme, invalid config: '{err}'");
                        continue;
                    }
                    config.active_panes =
                        Config::calc_active_panes(&config.tabs.tabs, &config.theme.layout);
                    ctx.config = Arc::new(config);

                    if let Err(err) = ui.on_event(UiEvent::ConfigChanged, &mut ctx) {
                        log::error!(error:? = err; "UI failed to handle config changed event");
                    }

                    // Need to clear the terminal to avoid artifacts from album art and other
                    // elements
                    if let Err(err) = terminal.clear() {
                        log::error!(error:? = err; "Failed to clear terminal after config change");
                        continue;
                    }
                    render_wanted = true;
                }
                AppEvent::UserKeyInput(key) => match ui.handle_key(&mut key.into(), &mut ctx) {
                    Ok(KeyHandleResult::None) => continue,
                    Ok(KeyHandleResult::Quit) => {
                        if let Err(err) = ui.on_event(UiEvent::Exit, &mut ctx) {
                            log::error!(error:? = err, event:?; "UI failed to handle quit event");
                        }
                        break;
                    }
                    Err(err) => {
                        status_error!(err:?; "Error: {}", err.to_status());
                        render_wanted = true;
                    }
                },
                AppEvent::UserMouseInput(ev) => match ui.handle_mouse_event(ev, &mut ctx) {
                    Ok(()) => {}
                    Err(err) => {
                        status_error!(err:?; "Error: {}", err.to_status());
                        render_wanted = true;
                    }
                },
                AppEvent::Status(mut message, level, timeout) => {
                    ctx.messages.push(StatusMessage {
                        level,
                        timeout,
                        message: std::mem::take(&mut message),
                        created: std::time::Instant::now(),
                    });

                    render_wanted = true;
                    // Send delayed render event to make the status message
                    // disappear
                    ctx.scheduler
                        .schedule(timeout, |(tx, _)| Ok(tx.send(AppEvent::RequestRender)?));
                }
                AppEvent::InfoModal { message, title, size, replacement_id: id } => {
                    if let Err(err) = ui.on_ui_app_event(
                        UiAppEvent::Modal(Box::new(
                            InfoModal::builder()
                                .ctx(&ctx)
                                .maybe_title(title)
                                .maybe_size(size)
                                .maybe_replacement_id(id)
                                .message(message)
                                .build(),
                        )),
                        &mut ctx,
                    ) {
                        log::error!(error:? = err; "UI failed to handle modal event");
                    }
                }
                AppEvent::Log(msg) => {
                    if let Err(err) = ui.on_event(UiEvent::LogAdded(msg), &mut ctx) {
                        log::error!(error:? = err; "UI failed to handle log event");
                    }
                }
                AppEvent::IdleEvent(event) => {
                    handle_idle_event(event, &ctx, &mut additional_evs);
                    for ev in additional_evs.drain() {
                        if let Err(err) = ui.on_event(ev, &mut ctx) {
                            status_error!(error:? = err, event:?; "UI failed to handle idle event, event: '{:?}', error: '{}'", event, err.to_status());
                        }
                    }
                    render_wanted = true;
                }
                AppEvent::RequestRender => {
                    render_wanted = true;
                }
                AppEvent::WorkDone(Ok(result)) => match result {
                    WorkDone::SearchYtResults { mut items, position } => {
                        let labels: Vec<String> = items
                            .iter()
                            .map(|it| it.title.as_deref().unwrap_or("<no title>").to_string())
                            .collect();

                        let modal = SelectModal::builder()
                            .ctx(&ctx)
                            .title("Search results")
                            .confirm_label("Select")
                            .options(labels)
                            .on_confirm(move |ctx, _label, idx| {
                                let url = std::mem::take(&mut items[idx].url);
                                if let Err(e) = ctx
                                    .work_sender
                                    .send(WorkRequest::Command(Command::AddYt { url, position }))
                                {
                                    log::error!("Failed to send enqueue command: {e}");
                                }
                                Ok(())
                            })
                            .build();

                        if let Err(err) =
                            ui.on_ui_app_event(UiAppEvent::Modal(Box::new(modal)), &mut ctx)
                        {
                            log::error!(error:? = err; "UI failed to handle modal event");
                        }

                        render_wanted = true;
                    }
                    WorkDone::LyricsIndexed { index } => {
                        ctx.lrc_index = index;
                        if let Err(err) = ui.on_event(UiEvent::LyricsIndexed, &mut ctx) {
                            log::error!(error:? = err; "UI failed to handle lyrics indexed event");
                        }
                    }
                    WorkDone::SingleLrcIndexed { lrc_entry } => {
                        if let Some(lrc_entry) = lrc_entry {
                            ctx.lrc_index.add(lrc_entry);
                        }
                        if let Err(err) = ui.on_event(UiEvent::LyricsIndexed, &mut ctx) {
                            log::error!(error:? = err; "UI failed to handle single lyrics indexed event");
                        }
                    }
                    WorkDone::QueryFinished { id, target, data } => match (id, target, data) {
                        (GLOBAL_STICKERS_UPDATE, None, QueryResult::SongStickers(stickers)) => {
                            ctx.set_stickers(stickers);
                            render_wanted = true;
                        }
                        (
                            GLOBAL_STATUS_UPDATE,
                            None,
                            QueryResult::Status { data: status, source_event },
                        ) => {
                            let current_song_id =
                                ctx.find_current_song_in_queue().map(|(_, song)| song.id);
                            let previous_state = ctx.status.state;
                            let current_updating_db = ctx.status.updating_db;
                            let current_playlist = ctx.status.lastloadedplaylist.take();
                            let previous_status = std::mem::replace(&mut ctx.status, status);
                            let new_playlist = ctx.status.lastloadedplaylist.as_ref();
                            let mut song_changed = false;

                            if ctx.config.reflect_changes_to_playlist
                                && matches!(source_event, Some(IdleEvent::Playlist))
                            {
                                // Try to reflect changes to saved playlist if any was loaded both
                                // before and after the update
                                if let (Some(current_playlist), Some(new_playlist)) =
                                    (current_playlist, new_playlist)
                                    && &current_playlist == new_playlist
                                {
                                    let playlist_name = current_playlist.clone();
                                    ctx.command(move |client| {
                                        client.save_queue_as_playlist(
                                            &playlist_name,
                                            Some(SaveMode::Replace),
                                        )?;
                                        Ok(())
                                    });
                                }
                            }

                            let mut start_render_loop = || {
                                _update_db_loop_guard = Some(ctx.scheduler.repeated(
                                    Duration::from_secs(1),
                                    |(tx, _)| {
                                        tx.send(AppEvent::RequestRender)?;
                                        Ok(())
                                    },
                                ));
                            };
                            match (current_updating_db, ctx.status.updating_db) {
                                (None, Some(_)) => {
                                    // update of db started
                                    ctx.db_update_start = Some(std::time::Instant::now());
                                    start_render_loop();
                                }
                                (Some(_), Some(_)) if ctx.db_update_start.is_none() => {
                                    // rmpc is opened after db started updating
                                    // beforehand so we reassign
                                    ctx.db_update_start = Some(std::time::Instant::now());
                                    start_render_loop();
                                }
                                (Some(_), None) => {
                                    // update of db ended
                                    ctx.db_update_start = None;
                                    _update_db_loop_guard = None;
                                }
                                _ => {}
                            }

                            if previous_state != ctx.status.state
                                && let Err(err) =
                                    ui.on_event(UiEvent::PlaybackStateChanged, &mut ctx)
                            {
                                status_error!(error:? = err; "UI failed to handle playback state changed event, error: '{}'", err.to_status());
                            }

                            // For YouTube backend: Use adaptive polling intervals
                            // Playing = fast polling (1000ms), Paused/Stopped = slow polling
                            // (5000ms)
                            let is_youtube =
                                matches!(ctx.config.backend, crate::config::PlayerBackend::YouTube);

                            match ctx.status.state {
                                State::Play if previous_state == ctx.status.state => {
                                    if let Some(played) = &mut ctx.song_played {
                                        *played += ctx.last_status_update.elapsed();
                                    }
                                }
                                State::Play if previous_state != ctx.status.state => {
                                    // Transitioning TO Play state
                                    if is_youtube {
                                        // YouTube: Switch to fast polling (1000ms)
                                        let interval =
                                            ctx.config.status_update_interval_ms.unwrap_or(1000);
                                        _update_loop_guard = Some(ctx.scheduler.repeated(
                                            Duration::from_millis(interval),
                                            run_status_update,
                                        ));
                                        log::debug!(
                                            "YouTube: switched to fast polling ({}ms)",
                                            interval
                                        );
                                    } else {
                                        // MPD: Start the timer when transitioning to Play
                                        _update_loop_guard = ctx
                                            .config
                                            .status_update_interval_ms
                                            .map(Duration::from_millis)
                                            .map(|interval| {
                                                ctx.scheduler.repeated(interval, run_status_update)
                                            });
                                    }

                                    // Start render timer for progress bar updates (1 second)
                                    // This is SEPARATE from status polling - just triggers
                                    // re-render
                                    _playback_render_guard = Some(ctx.scheduler.repeated(
                                        Duration::from_secs(1),
                                        |(tx, _)| {
                                            tx.send(AppEvent::RequestRender)?;
                                            Ok(())
                                        },
                                    ));
                                }
                                State::Play => {}
                                State::Pause => {
                                    // Stop progress bar render timer (playback paused)
                                    _playback_render_guard = None;

                                    if is_youtube {
                                        // YouTube: Switch to slow polling (5000ms) - save CPU
                                        _update_loop_guard = Some(ctx.scheduler.repeated(
                                            Duration::from_millis(5000),
                                            run_status_update,
                                        ));
                                        log::debug!("YouTube: switched to slow polling (5000ms)");
                                    } else {
                                        // MPD: stop polling on pause
                                        _update_loop_guard = None;
                                    }
                                }
                                State::Stop => {
                                    song_changed = true;
                                    ctx.song_played = None;
                                    // Stop progress bar render timer
                                    _playback_render_guard = None;

                                    if is_youtube {
                                        // YouTube: Switch to slow polling (5000ms) - save CPU
                                        _update_loop_guard = Some(ctx.scheduler.repeated(
                                            Duration::from_millis(5000),
                                            run_status_update,
                                        ));
                                        log::debug!("YouTube: switched to slow polling (5000ms)");
                                    } else {
                                        // MPD: stop polling on stop
                                        _update_loop_guard = None;
                                    }
                                }
                            }

                            if let Some((_, song)) = ctx.find_current_song_in_queue()
                                && Some(song.id) != current_song_id
                            {
                                if let Some(command) = &ctx.config.on_song_change {
                                    let mut env = create_env(&ctx, std::iter::empty());

                                    let prev_song_file = (previous_status.state != State::Stop)
                                        .then(|| {
                                            previous_status.songid.and_then(|id| {
                                                ctx.queue_store()
                                                    .read()
                                                    .iter()
                                                    .find(|song| song.id == Some(id))
                                                    .map(|s| s.uri.clone())
                                            })
                                        })
                                        .flatten();

                                    if let (Some(prev_song), Some(played)) =
                                        (prev_song_file, ctx.song_played)
                                    {
                                        env.push(("PREV_SONG".to_owned(), prev_song));
                                        env.push((
                                            "PREV_ELAPSED".to_owned(),
                                            played.as_secs().to_string(),
                                        ));
                                    }

                                    run_external(command.clone(), env);
                                }
                                song_changed = true;
                                ctx.song_played = Some(Duration::ZERO);
                            }
                            if song_changed
                                && let Err(err) = ui.on_event(UiEvent::SongChanged, &mut ctx)
                            {
                                status_error!(error:? = err; "UI failed to handle idle event, error: '{}'", err.to_status());
                            }

                            ctx.last_status_update = Instant::now();

                            // Smart render decision: only re-render if something visually changed
                            // For YouTube backend, we poll frequently but most polls return same
                            // data Comparing elapsed time would always
                            // trigger render during playback
                            // So we compare everything EXCEPT elapsed (which is interpolated
                            // client-side)
                            let needs_render = previous_state != ctx.status.state  // play/pause/stop changed
                                || previous_status.volume != ctx.status.volume  // volume changed
                                || previous_status.repeat != ctx.status.repeat  // repeat changed
                                || previous_status.random != ctx.status.random  // shuffle changed
                                || previous_status.single != ctx.status.single  // single changed
                                || previous_status.songid != ctx.status.songid  // song changed
                                || previous_status.playlistlength != ctx.status.playlistlength  // queue changed
                                || previous_status.duration != ctx.status.duration  // song duration changed
                                || song_changed; // song actually changed

                            if needs_render {
                                log::debug!("Status change requires render");
                                render_wanted = true;
                            }
                        }
                        ("global_volume_update", None, QueryResult::Volume(volume)) => {
                            ctx.status.volume = volume.0 as u8;
                            render_wanted = true;
                        }
                        // Handle Queue result from ANY query - result type determines state update
                        (id, _, QueryResult::Queue(queue)) => {
                            let queue = queue.unwrap_or_default();
                            ctx.queue_store().reconcile(queue);
                            render_wanted = true;
                            log::debug!(id, len = ctx.queue_store().len(); "Queue updated");
                            if let Err(err) = ui.on_event(UiEvent::QueueChanged, &mut ctx) {
                                status_error!(error:? = err; "Ui failed to handle queue changed event, error: '{}'", err.to_status());
                            }
                        }
                        (EXTERNAL_COMMAND, None, QueryResult::ExternalCommand(command, songs)) => {
                            let songs = songs.iter().map(|s| s.uri.as_str());
                            run_external(command, create_env(&ctx, songs));
                        }
                        (id, target, data) => {
                            if let Err(err) = ui.on_command_finished(id, target, data, &mut ctx) {
                                log::error!(error:? = err; "UI failed to handle command finished event");
                            }
                        }
                    },
                    WorkDone::None => {}
                },
                AppEvent::WorkDone(Err(err)) => {
                    status_error!("{}", err);
                }
                AppEvent::Resized { columns, rows } => {
                    ctx.scheduler.schedule_replace(
                        *ON_RESIZE_SCHEDULE_ID,
                        Duration::from_millis(500),
                        move |(tx, _)| {
                            tx.send(AppEvent::ResizedDebounced { columns, rows })?;
                            Ok(())
                        },
                    );
                    render_wanted = true;
                }
                AppEvent::ResizedDebounced { columns, rows } => {
                    if let Err(err) = ui.resize(Rect::new(0, 0, columns, rows), &ctx) {
                        log::error!(error:? = err, event:?; "UI failed to handle resize event");
                    }

                    if let Some(cmd) = &ctx.config.on_resize {
                        let cmd = Arc::clone(cmd);
                        let mut env = create_env(&ctx, std::iter::empty::<&str>());
                        env.push(("COLS".to_owned(), columns.to_string()));
                        env.push(("ROWS".to_owned(), rows.to_string()));
                        log::debug!("Executing on resize");
                        run_external(cmd, env);
                    }
                    if let Err(err) = terminal.clear() {
                        log::error!(error:? = err; "Failed to clear terminal after a resize");
                    }
                    render_wanted = true;
                }
                AppEvent::UiEvent(event) => match ui.on_ui_app_event(event, &mut ctx) {
                    Ok(()) => {}
                    Err(err) => {
                        status_error!(err:?; "Error: {}", err.to_status());
                        render_wanted = true;
                    }
                },
                AppEvent::RemoteSwitchTab { tab_name } => {
                    let target_tab = tab_name.as_str().into();

                    if let Some(tab) =
                        ctx.config.tabs.names.iter().find(|&name| *name == target_tab)
                    {
                        if let Err(err) =
                            ui.on_ui_app_event(UiAppEvent::ChangeTab(tab.clone()), &mut ctx)
                        {
                            status_error!(err:?; "Error switching to tab '{}': {}", tab_name, err.to_status());
                        }
                    } else {
                        let available = ctx
                            .config
                            .tabs
                            .names
                            .iter()
                            .map(|name| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        status_error!(
                            "Tab '{}' does not exist. Available tabs: {}",
                            tab_name,
                            available
                        );
                    }
                    render_wanted = true;
                }
                AppEvent::IpcQuery { mut stream, targets } => {
                    for target in targets {
                        match target {
                            RemoteCommandQuery::ActiveTab => {
                                stream
                                    .insert_response(target.to_string(), ctx.active_tab.0.as_str());
                            }
                        }
                    }
                }
                AppEvent::Reconnected => {
                    for ev in [IdleEvent::Player, IdleEvent::Playlist, IdleEvent::Options] {
                        handle_idle_event(ev, &ctx, &mut additional_evs);
                    }
                    if let Err(err) = ui.on_event(UiEvent::Reconnected, &mut ctx) {
                        log::error!(error:? = err, event:?; "UI failed to handle resize event");
                    }
                    status_warn!("rmpc reconnected to MPD and will reinitialize");
                    connected = true;
                }
                AppEvent::LostConnection => {
                    if ctx.status.state != State::Stop {
                        _update_loop_guard = None;
                        ctx.status.state = State::Stop;
                    }
                    if connected {
                        status_error!("rmpc lost connection to MPD and will try to reconnect");
                    }
                    connected = false;
                }
                AppEvent::TmuxHook { hook } => {
                    if let Some(tmux) = &mut tmux {
                        let old_visible = tmux.visible;
                        if let Err(err) = tmux.update_visible() {
                            log::error!(err:?, hook:?; "Failed to update tmux visibility");
                            continue;
                        }

                        let event = match (tmux.visible, old_visible) {
                            (true, false) => UiEvent::Displayed,
                            (false, true) => UiEvent::Hidden,
                            _ => continue,
                        };

                        match ui.on_event(event, &mut ctx) {
                            Ok(()) => {}
                            Err(err) => {
                                status_error!(err:?; "Error: {}", err.to_status());
                                render_wanted = true;
                            }
                        }
                    }
                }
            }
        }
        if render_wanted {
            let till_next_frame =
                min_frame_duration.saturating_sub(now.duration_since(last_render));
            if till_next_frame != Duration::ZERO {
                continue;
            }

            // DEBUG: Track render frequency
            static RENDER_COUNT: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            static LAST_LOG: std::sync::LazyLock<std::sync::Mutex<Instant>> =
                std::sync::LazyLock::new(|| std::sync::Mutex::new(Instant::now()));

            let count = RENDER_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut last) = LAST_LOG.try_lock() {
                if last.elapsed() >= Duration::from_secs(5) {
                    log::warn!(
                        "Render stats: {} renders in last 5 seconds ({:.1} FPS)",
                        count,
                        count as f64 / 5.0
                    );
                    RENDER_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
                    *last = Instant::now();
                }
            }

            terminal
                .draw(|frame| {
                    if let Err(err) = ui.render(frame, &mut ctx) {
                        log::error!(error:? = err; "Failed to render a frame");
                    }
                })
                .expect("Expected render to succeed");

            ctx.finish_frame();
            last_render = now;
            render_wanted = false;
        }
    }

    terminal
}

fn handle_idle_event(event: IdleEvent, ctx: &Ctx, result_ui_evs: &mut HashSet<UiEvent>) {
    match event {
        IdleEvent::Mixer if ctx.supported_commands.contains("getvol") => {
            ctx.query().id(GLOBAL_VOLUME_UPDATE).replace_id("volume").query(move |client| {
                let status = client.get_status()?;
                Ok(QueryResult::Volume(Volume::new(status.volume as u32)))
            });
        }
        IdleEvent::Mixer => {
            ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(move |client| {
                Ok(QueryResult::Status {
                    data: client.get_status()?,
                    source_event: Some(IdleEvent::Mixer),
                })
            });
        }
        IdleEvent::Options => {
            ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(move |client| {
                Ok(QueryResult::Status {
                    data: client.get_status()?,
                    source_event: Some(IdleEvent::Options),
                })
            });
        }
        IdleEvent::Player => {
            ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(move |client| {
                Ok(QueryResult::Status {
                    data: client.get_status()?,
                    source_event: Some(IdleEvent::Player),
                })
            });
        }
        IdleEvent::Playlist => {
            log::debug!("Queue update requested");
            let app_state = ctx.app_state.clone();
            ctx.query().id(GLOBAL_QUEUE_UPDATE).replace_id("playlist").query(move |client| {
                let queue = client.playlist_info()?;
                Ok(QueryResult::Queue(Some(queue)))
            });
            if ctx.config.reflect_changes_to_playlist {
                // Do not replace because we want to update currently loaded playlist if any
                ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status_from_playlist").query(
                    move |client| {
                        Ok(QueryResult::Status {
                            data: client.get_status()?,
                            source_event: Some(IdleEvent::Playlist),
                        })
                    },
                );
            }
        }
        IdleEvent::Sticker => {
            if ctx.stickers_supported.into() {
                let songs: Vec<_> = ctx.stickers().keys().cloned().collect();
                ctx.query().id(GLOBAL_STICKERS_UPDATE).replace_id("global_stickers_update").query(
                    move |client| Ok(QueryResult::SongStickers(client.fetch_song_stickers(songs)?)),
                );
            }
        }
        IdleEvent::StoredPlaylist => {}
        IdleEvent::Database => {
            ctx.query().id(GLOBAL_STATUS_UPDATE).replace_id("status").query(move |client| {
                Ok(QueryResult::Status {
                    data: client.get_status()?,
                    source_event: Some(IdleEvent::Database),
                })
            });
        }
        IdleEvent::Update => {}
        IdleEvent::Output => {}
        IdleEvent::Partition
        | IdleEvent::Subscription
        | IdleEvent::Message
        | IdleEvent::Neighbor
        | IdleEvent::Mount => {
            log::warn!(event:?; "Received unhandled event");
        }
    }

    if let Ok(ev) = event.try_into() {
        result_ui_evs.insert(ev);
    }
}
