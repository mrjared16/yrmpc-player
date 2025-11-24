use anyhow::Result;
use ratatui::{Frame, prelude::Rect, widgets::ListState};

use super::Pane;
use crate::{
    MpdQueryResult,
    config::tabs::PaneType,
    ctx::Ctx,
    domain::Song,
    mpd::commands::lsinfo::LsInfoEntry,
    player::Client,
    shared::{
        key_event::KeyEvent,
        mouse_event::MouseEvent,
    },
    ui::{
        UiEvent,
        browser::BrowserPane,
        dir_or_song::DirOrSong,
        dirstack::DirStack,
        widgets::browser::Browser,
    },
};

#[derive(Debug)]
pub struct LibraryPane {
    stack: DirStack<DirOrSong, ListState>,
    filter_input_mode: bool,
    browser: Browser<DirOrSong>,
    initialized: bool,
}

const INIT: &str = "init";
const FETCH_DATA: &str = "fetch_data";

impl LibraryPane {
    pub fn new(_ctx: &Ctx) -> Self {
        Self {
            stack: DirStack::default(),
            filter_input_mode: false,
            browser: Browser::new(),
            initialized: false,
        }
    }
}

impl Pane for LibraryPane {
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
            let target = PaneType::Library;
            // Initialize with category list
            ctx.query().id(INIT).replace_id(INIT).target(target).query(move |_client| {
                let categories = vec![
                    "library:playlists".to_string(),
                    "library:albums".to_string(), 
                    "library:artists".to_string(),
                    "library:songs".to_string(),
                ];
                Ok(MpdQueryResult::LsInfo { data: categories, path: None })
            });

            self.initialized = true;
        }

        Ok(())
    }

    fn on_event(&mut self, event: &mut UiEvent, _is_visible: bool, ctx: &Ctx) -> Result<()> {
        match event {
            UiEvent::Database => {
                self.stack = DirStack::default();
                self.initialized = false;
                self.before_show(ctx)?;
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
        data: MpdQueryResult,
        _is_visible: bool,
        ctx: &Ctx,
    ) -> Result<()> {
        match (id, data) {
            (INIT, MpdQueryResult::LsInfo { data, path: _ }) => {
                let data: Vec<DirOrSong> = data.into_iter().map(DirOrSong::name_only).collect();
                self.stack = DirStack::new(data);
                ctx.render()?;
            }
            (FETCH_DATA, MpdQueryResult::DirOrSong { data, path }) => {
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

impl BrowserPane<DirOrSong> for LibraryPane {
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
    ) -> impl FnOnce(&mut Client<'_>) -> Result<Vec<Song>> + Clone + 'static {
        move |client| {
            Ok(match item {
                DirOrSong::Dir { name, .. } => {
                    // For library items, use lsinfo with "library:category" prefix
                    if name.starts_with("library:") || name.starts_with("playlist:") || name.starts_with("album:") || name.starts_with("artist:") {
                        client.lsinfo(Some(&name))?
                            .into_iter()
                            .filter_map(|entry| match entry {
                                LsInfoEntry::File(song) => Some(song.into()),
                                _ => None,
                            })
                            .collect()
                    } else {
                        vec![]
                    }
                }
                DirOrSong::Song(song) => vec![song.clone()],
            })
        }
    }

    fn fetch_data(&self, selected: &DirOrSong, ctx: &Ctx) -> Result<()> {
        let _current_path = self.stack.path().to_owned();

        if let DirOrSong::Dir { name, .. } = selected {
            if name.starts_with("library:") || name.starts_with("playlist:") || name.starts_with("album:") || name.starts_with("artist:") {
                let name_clone = name.clone();
                let target = PaneType::Library;
                let next_path = self.stack.path().join(name.clone());

                ctx.query().id(FETCH_DATA).replace_id(FETCH_DATA).target(target).query(move |client| {
                    let result = client.lsinfo(Some(&name_clone))?;
                    let mapped: Vec<DirOrSong> = result.into_iter().filter_map(|entry| {
                        match entry {
                            LsInfoEntry::File(song) => Some(DirOrSong::Song(song.into())),
                            LsInfoEntry::Dir(dir) => Some(DirOrSong::Dir {
                                name: dir.name.clone(),
                                full_path: dir.full_path,
                                playlist: false,
                                last_modified: dir.last_modified,
                            }),
                            _ => None,
                        }
                    }).collect();
                    Ok(MpdQueryResult::DirOrSong { data: mapped, path: Some(next_path) })
                });
                return Ok(());
            }
        }

        ctx.render()?;
        Ok(())
    }
}
