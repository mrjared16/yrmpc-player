use std::{cmp::Ordering, sync::Arc};

use anyhow::{Context, Result};
use enum_map::EnumMap;
use itertools::Itertools;
use ratatui::{Frame, prelude::Rect, widgets::ListState};

use super::Pane;
use crate::{
    QueryResult,
    config::{
        artists::{AlbumDisplayMode, AlbumSortMode},
        tabs::PaneType,
    },
    ctx::Ctx,
    domain::Song,
    mpd::{
        commands::lsinfo::LsInfoEntry,
        mpd_client::{Filter, FilterKind, Tag},
    },
    backends::BackendDispatcher,
    shared::{
        cmp::StringCompare,
        key_event::KeyEvent,
        mouse_event::MouseEvent,
    },
    ui::{
        UiEvent,
        browser::BrowserPane,
        dir_or_song::DirOrSong,
        dirstack::{DirStack, DirStackItem, Path},
        widgets::browser::{Browser, BrowserArea},
    },
};

#[derive(Debug)]
pub struct ArtistPane {
    stack: DirStack<DirOrSong, ListState>,
    filter_input_mode: bool,
    root_tag: Tag,
    separator: Option<Arc<str>>,
    unescaped_separator: Option<String>,
    target_pane: PaneType,
    browser: Browser<DirOrSong>,
    initialized: bool,
}

const INIT: &str = "init";
const FETCH_SONGS: &str = "fetch_songs";
const FETCH_DATA: &str = "fetch_data";

impl ArtistPane {
    pub fn new(
        _ctx: &Ctx,
    ) -> Self {
        Self {
            root_tag: Tag::Artist,
            target_pane: PaneType::Artists,
            separator: None,
            unescaped_separator: None,
            stack: DirStack::default(),
            filter_input_mode: false,
            browser: Browser::new(),
            initialized: false,
        }
    }

    fn root_tag_filter<'value>(
        root_tag: Tag,
        separator: Option<&str>,
        value: &'value str,
    ) -> Filter<'value> {
        match separator {
            None => Filter::new(root_tag, value),
            Some(_) if value.is_empty() => Filter::new(root_tag, value),
            Some(separator) => Filter::new_with_kind(
                root_tag,
                format!("(^|.*{separator}){value}($|{separator}.*)"),
                FilterKind::Regex,
            ),
        }
    }

    fn process_songs(&mut self, artist: String, data: Vec<Song>, ctx: &Ctx) {
        // This is used when we are browsing "Artist -> Album".
        // But for "Rich Artist View", we might receive a flat list of songs/albums from `lsinfo`.
        // If we use the standard `TagBrowserPane` logic, it groups songs by album.
        
        // For now, let's keep the standard logic for compatibility, 
        // but we will override `fetch_data` to handle "artist:ID" paths.
        
        let display_mode = ctx.config.artists.album_display_mode;
        let sort_mode = ctx.config.artists.album_sort_by;

        let albums = data
            .into_iter()
            .into_group_map_by(|song| {
                let album = song.metadata.get("album").map_or("<no album>".to_string(), |v| {
                    v.join(&ctx.config.theme.format_tag_separator).to_string()
                });
                let song_date = ctx
                    .config
                    .artists
                    .album_date_tags
                    .iter()
                    .find_map(|tag| {
                        song.metadata
                            .get(Into::<&'static str>::into(tag))
                            .and_then(|v| v.last().map(|s| s.clone()))
                    })
                    .unwrap_or_else(|| "<no date>".to_string());

                (album, song_date)
            })
            .into_iter()
            .sorted_by(|((album_a, date_a), _), ((album_b, date_b), _)| match sort_mode {
                AlbumSortMode::Name => match album_a.cmp(album_b) {
                    Ordering::Equal => date_a.cmp(date_b),
                    ordering => ordering,
                },
                AlbumSortMode::Date => date_a.cmp(date_b),
            })
            .map(|((album, date), mut songs)| {
                songs.sort_by(|a, b| {
                    a.with_custom_sort(&ctx.config.browser_song_sort)
                        .cmp(&b.with_custom_sort(&ctx.config.browser_song_sort))
                });

                let name = match display_mode {
                    AlbumDisplayMode::SplitByDate => {
                        format!("({date}) {album}")
                    }
                    AlbumDisplayMode::NameOnly => album.clone(),
                };
                (name, songs)
            })
            .fold(Vec::new(), |mut acc, album| {
                match display_mode {
                    AlbumDisplayMode::SplitByDate => {
                        acc.push(album);
                    }
                    AlbumDisplayMode::NameOnly => {
                        if let Some(cached_album) =
                            acc.iter_mut().find(|cached_album| cached_album.0 == album.0)
                        {
                            cached_album.1.extend(album.1);
                        } else {
                            acc.push(album);
                        }
                    }
                }
                acc
            });

        let path: Path = artist.into();
        self.stack.insert(
            path.clone(),
            albums.iter().map(|album| DirOrSong::name_only(album.0.clone())).collect(),
        );

        for album in albums {
            let album_path = path.join(album.0);
            self.stack.insert(album_path, album.1.into_iter().map(DirOrSong::Song).collect());
        }
    }
}

impl Pane for ArtistPane {
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        // TODO: Check stack depth and render Rich View if depth == 1 (Artist selected)
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
            let root_tag = self.root_tag.clone();
            let target = self.target_pane.clone();
            ctx.query().id(INIT).replace_id(INIT).target(target).query(move |client| {
                let result = client.list_tag(root_tag, None).context("Cannot list artists")?;
                Ok(QueryResult::LsInfo { data: result, path: None })
            });

            self.initialized = true;
        }

        Ok(())
    }

    fn on_event(&mut self, event: &mut UiEvent, _is_visible: bool, ctx: &Ctx) -> Result<()> {
        match event {
            UiEvent::Database => {
                let root_tag = self.root_tag.clone();
                let target = self.target_pane.clone();
                self.stack = DirStack::default();
                ctx.query().id(INIT).replace_id(INIT).target(target).query(move |client| {
                    let result = client.list_tag(root_tag, None).context("Cannot list artists")?;
                    Ok(QueryResult::LsInfo { data: result, path: None })
                });
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
            (FETCH_SONGS, QueryResult::SongsList { data, path }) => {
                let Some(root_path) = path.and_then(|v| v.as_slice().iter().next().cloned()) else {
                    return Ok(());
                };

                self.process_songs(root_path, data, ctx);
                self.fetch_data_internal(ctx)?;
                ctx.render()?;
            }
            (INIT, QueryResult::LsInfo { data, path: _ }) => {
                let sort_opts = ctx.config.browser_song_sort.as_ref();

                let data = if let Some(sep) = &self.unescaped_separator {
                    data.into_iter()
                        .flat_map(|item| item.split(sep.as_str()).map(str::to_string).collect_vec())
                        .unique()
                        .sorted_by(|a, b| StringCompare::from(sort_opts).compare(a, b))
                        .map(DirOrSong::name_only)
                        .collect_vec()
                } else {
                    data.into_iter()
                        .sorted_by(|a, b| StringCompare::from(sort_opts).compare(a, b))
                        .map(DirOrSong::name_only)
                        .collect_vec()
                };

                self.stack = DirStack::new(data);
                if let Some(sel) = self.stack.current().selected() {
                    self.fetch_data(sel, ctx)?;
                }
                ctx.render()?;
            }
            (FETCH_DATA, QueryResult::DirOrSong { data, path }) => {
                 // This handles the result from lsinfo("artist:ID")
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

impl BrowserPane<DirOrSong> for ArtistPane {
    fn stack(&self) -> &DirStack<DirOrSong, ListState> {
        &self.stack
    }

    fn stack_mut(&mut self) -> &mut DirStack<DirOrSong, ListState> {
        &mut self.stack
    }

    fn browser_areas(&self) -> EnumMap<BrowserArea, Rect> {
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
        let root_tag = self.root_tag.clone();
        let separator = self.separator.clone();
        let path = self.stack().path().to_owned();

        let album_songs = match self.stack.path().as_slice() {
            [_artist] => self
                .stack
                .next_dir_items()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| match item {
                            DirOrSong::Dir { .. } => None,
                            DirOrSong::Song(song) => Some(song.clone()),
                        })
                        .collect()
                    })
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        move |client| {
            Ok(match item {
                DirOrSong::Dir { name, .. } => match path.as_slice() {
                    [_artist] => album_songs,
                    // Convert MediaItem to Song for legacy pane compatibility
                    [] => client.find(
                        &[Self::root_tag_filter(root_tag, separator.as_deref(), &name)],
                        None,
                    )?.into_iter().map(Song::from).collect(),
                    _ => Vec::new(),
                },
                DirOrSong::Song(song) => vec![song.clone()],
            })
        }
    }

    fn fetch_data(&self, selected: &DirOrSong, ctx: &Ctx) -> Result<()> {
        // Here we intercept the fetch to handle "artist:ID"
        let current_path = self.stack.path().to_owned();
        
        // If we are at root and selected an item
        if current_path.is_empty() {
             if let DirOrSong::Dir { name, .. } = selected {
                 if name.starts_with("artist:") {
                     // It's a YT Music artist, use lsinfo
                     let name_clone = name.clone();
                     let target = self.target_pane.clone();
                     let next_path = self.stack.path().join(name.clone());
                     
                     ctx.query().id(FETCH_DATA).replace_id(FETCH_DATA).target(target).query(move |client| {
                         let result = client.lsinfo(Some(&name_clone))?;
                         // Convert Vec<LsInfoEntry> to Vec<DirOrSong>
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
                         Ok(QueryResult::DirOrSong { data: mapped, path: Some(next_path) })
                     });
                     return Ok(());
                 }
             }
        }
        
        // Fallback to default behavior
        match self.stack.path().as_slice() {
            [_artist, _album] => {
                ctx.render()?;
            }
            [_artist] => {
                ctx.render()?;
            }
            [] => {
                let current = selected.as_path();
                let root_tag = self.root_tag.clone();
                let separator = self.separator.clone();
                let target = self.target_pane.clone();
                let current = current.to_owned();

                ctx.query().id(FETCH_SONGS).replace_id(FETCH_SONGS).target(target).query(
                    move |client| {
                        let separator = separator.map(|v| v.as_ref().to_owned());
                        let separator = separator.as_deref();
                        // Convert MediaItem to Song for legacy pane compatibility
                        let all_songs: Vec<Song> = client
                            .find(&[Self::root_tag_filter(root_tag, separator, &current)], None)?
                            .into_iter().map(Song::from).collect();
                        Ok(QueryResult::SongsList {
                            data: all_songs,
                            path: Some(current.into()),
                        })
                    },
                );
            }
            _ => {}
        }
        Ok(())
    }
}
