use anyhow::Result;
use ratatui::{Frame, prelude::Rect, widgets::ListState};

use super::Pane;
use crate::{
    QueryResult,
    backends::BackendDispatcher,
    config::tabs::PaneType,
    ctx::Ctx,
    domain::Song,
    mpd::commands::lsinfo::LsInfoEntry,
    shared::{key_event::KeyEvent, mouse_event::MouseEvent},
    ui::{
        UiEvent, browser::BrowserPane, dir_or_song::DirOrSong, dirstack::DirStack,
        widgets::browser::Browser,
    },
};

#[derive(Debug)]
pub struct PlaylistPane {
    stack: DirStack<DirOrSong, ListState>,
    filter_input_mode: bool,
    browser: Browser<DirOrSong>,
    initialized: bool,
}

const INIT: &str = "init";
const FETCH_DATA: &str = "fetch_data";

impl PlaylistPane {
    pub fn new(_ctx: &Ctx) -> Self {
        Self {
            stack: DirStack::default(),
            filter_input_mode: false,
            browser: Browser::new(),
            initialized: false,
        }
    }
}

impl Pane for PlaylistPane {
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        self.browser.set_filter_input_active(self.filter_input_mode).render(
            area,
            frame.buffer_mut(),
            &mut self.stack,
            ctx,
        );

        Ok(())
    }

    fn before_show(&mut self, ctx: &Ctx) -> Result<()> {
        if !self.initialized {
            let target = PaneType::Playlists;
            ctx.query().id(INIT).replace_id(INIT).target(target).query(move |_client| {
                // For now, just return empty list - playlists will be populated from search or
                // library
                Ok(QueryResult::LsInfo { data: vec![], path: None })
            });

            self.initialized = true;
        }

        Ok(())
    }

    fn on_event(&mut self, event: &mut UiEvent, _is_visible: bool, ctx: &Ctx) -> Result<()> {
        match event {
            UiEvent::Database => {
                self.stack = DirStack::default();
                // No init query needed - playlists come from search/library
                ctx.render()?;
            }
            UiEvent::Reconnected => {
                self.initialized = false;
                self.before_show(ctx)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, event: MouseEvent, ctx: &Ctx) -> Result<()> {
        self.handle_mouse_action(event, ctx)
    }

    fn handle_action(&mut self, event: &mut KeyEvent, ctx: &mut Ctx) -> Result<()> {
        self.handle_filter_input(event, ctx)?;
        self.handle_common_action(event, ctx)?;
        self.handle_global_action(event, ctx)?;
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
            (INIT, QueryResult::LsInfo { data, path: _ }) => {
                let data: Vec<DirOrSong> = data.into_iter().map(DirOrSong::name_only).collect();
                self.stack = DirStack::new(data);
                if let Some(sel) = self.stack.current().selected() {
                    self.fetch_data(sel, ctx)?;
                }
                ctx.render()?;
            }
            (FETCH_DATA, QueryResult::DirOrSong { data, path }) => {
                if let Some(path) = path {
                    self.stack.insert(path, data);
                    self.fetch_data_internal(ctx)?;
                    ctx.render()?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl BrowserPane<DirOrSong> for PlaylistPane {
    fn stack(&self) -> &DirStack<DirOrSong, ListState> {
        &self.stack
    }

    fn stack_mut(&mut self) -> &mut DirStack<DirOrSong, ListState> {
        &mut self.stack
    }

    fn browser_areas(&self) -> enum_map::EnumMap<crate::ui::widgets::browser::BrowserArea, Rect> {
        self.browser.areas
    }

    fn set_filter_input_mode_active(&mut self, active: bool) {
        self.filter_input_mode = active;
    }

    fn is_filter_input_mode_active(&self) -> bool {
        self.filter_input_mode
    }

    fn list_songs_in_item(
        &self,
        item: DirOrSong,
    ) -> impl FnOnce(&mut BackendDispatcher<'_>) -> Result<Vec<Song>> + Clone + 'static {
        move |client| {
            Ok(match item {
                DirOrSong::Dir { name, .. } => {
                    // For YouTube Music playlists, use lsinfo with "playlist:ID" prefix
                    if name.starts_with("playlist:") {
                        client
                            .lsinfo(Some(&name))?
                            .into_iter()
                            .filter_map(|entry| match entry {
                                LsInfoEntry::File(song) => Some(song.into()),
                                _ => None,
                            })
                            .collect()
                    } else {
                        // For MPD playlists, use listplaylistinfo
                        client.list_playlist_info(&name, None)?
                    }
                }
                DirOrSong::Song(song) => vec![song.clone()],
            })
        }
    }

    fn fetch_data(&self, selected: &DirOrSong, ctx: &Ctx) -> Result<()> {
        let current_path = self.stack.path().to_owned();

        if current_path.is_empty() {
            if let DirOrSong::Dir { name, .. } = selected {
                if name.starts_with("playlist:") {
                    // YouTube Music playlist - use lsinfo
                    let name_clone = name.clone();
                    let target = PaneType::Playlists;
                    let next_path = self.stack.path().join(name.clone());

                    ctx.query().id(FETCH_DATA).replace_id(FETCH_DATA).target(target).query(
                        move |client| {
                            let result = client.lsinfo(Some(&name_clone))?;
                            let mapped: Vec<DirOrSong> = result
                                .into_iter()
                                .filter_map(|entry| {
                                    match entry {
                                        LsInfoEntry::File(song) => {
                                            Some(DirOrSong::Song(song.into()))
                                        }
                                        LsInfoEntry::Dir(dir) => {
                                            // This line is syntactically incorrect as a field in a
                                            // struct literal.
                                            // Assuming it was meant to be a statement before the
                                            // struct construction,
                                            // but inserting it as requested by the user's provided
                                            // diff.
                                            // Note: `album_id` is not defined in this scope.
                                            Some(DirOrSong::Dir {
                                                // let _path = Path::from(album_id.clone());, //
                                                // This line is commented out as it causes a syntax
                                                // error.
                                                name: dir.name.clone(),
                                                full_path: dir.full_path,
                                                playlist: false,
                                                last_modified: dir.last_modified,
                                            })
                                        }
                                        _ => None,
                                    }
                                })
                                .collect();
                            Ok(QueryResult::DirOrSong { data: mapped, path: Some(next_path) })
                        },
                    );
                } else {
                    // MPD playlist - use listplaylistinfo
                    let name_clone = name.clone();
                    let target = PaneType::Playlists;
                    let next_path = self.stack.path().join(name.clone());

                    ctx.query().id(FETCH_DATA).replace_id(FETCH_DATA).target(target).query(
                        move |client| {
                            let songs = client.list_playlist_info(&name_clone, None)?;
                            let mapped: Vec<DirOrSong> = songs
                                .into_iter()
                                .map(|song| DirOrSong::Song(song.into()))
                                .collect();
                            Ok(QueryResult::DirOrSong { data: mapped, path: Some(next_path) })
                        },
                    );
                }
                return Ok(());
            }
        }

        ctx.render()?;
        Ok(())
    }
}
