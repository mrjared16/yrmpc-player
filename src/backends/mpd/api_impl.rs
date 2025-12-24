//! API trait implementations for MpdBackend.
//!
//! This file implements the backend-agnostic API traits for MPD,
//! translating between API types and MPD protocol types.

use std::time::Duration;
use anyhow::Result;

use super::backend::MpdBackend;
use super::protocol::mpd_client::MpdClient as MpdClientTrait;
use crate::backends::api::{
    self, Item, SearchQuery, SearchResults, BrowseResult, Capability,
    InsertAt, AfterAdd, ContentType,
};
use crate::mpd::commands::{LsInfoEntry, ValueChange};
use crate::mpd::mpd_client::{Filter, FilterKind, Tag};

impl api::Playback for MpdBackend<'_> {
    fn play(&mut self) -> Result<()> {
        self.client.play().map_err(Into::into)
    }

    fn pause(&mut self) -> Result<()> {
        self.client.pause().map_err(Into::into)
    }

    fn stop(&mut self) -> Result<()> {
        self.client.stop().map_err(Into::into)
    }

    fn next(&mut self) -> Result<()> {
        self.client.next().map_err(Into::into)
    }

    fn previous(&mut self) -> Result<()> {
        self.client.prev().map_err(Into::into)
    }

    fn seek(&mut self, position: Duration) -> Result<()> {
        self.client
            .seek_current(ValueChange::Set(position.as_secs() as u32))
            .map_err(Into::into)
    }

    fn seek_relative(&mut self, delta_secs: i64) -> Result<()> {
        let value_change = if delta_secs >= 0 {
            ValueChange::Increase(delta_secs as u32)
        } else {
            ValueChange::Decrease((-delta_secs) as u32)
        };
        
        self.client.seek_current(value_change).map_err(Into::into)
    }

    fn status(&mut self) -> Result<api::Status> {
        let mpd_status = self.client.get_status()?;
        
        let state = match mpd_status.state {
            crate::mpd::commands::status::State::Play => api::State::Playing,
            crate::mpd::commands::status::State::Pause => api::State::Paused,
            crate::mpd::commands::status::State::Stop => api::State::Stopped,
        };
        
        let repeat = match (mpd_status.repeat, mpd_status.single) {
            (true, crate::mpd::commands::OnOffOneshot::On) => api::Repeat::One,
            (true, _) => api::Repeat::All,
            (false, _) => api::Repeat::Off,
        };
        
        // MPD status has non-optional elapsed/duration fields
        let position = if mpd_status.elapsed.as_secs() > 0 || mpd_status.elapsed.subsec_nanos() > 0 {
            Some(mpd_status.elapsed)
        } else {
            None
        };
        
        let duration = if mpd_status.duration.as_secs() > 0 || mpd_status.duration.subsec_nanos() > 0 {
            Some(mpd_status.duration)
        } else {
            None
        };
        
        Ok(api::Status {
            state,
            position,
            duration,
            volume: mpd_status.volume.0 as u8,
            repeat,
            shuffle: mpd_status.random,
        })
    }
}

impl api::Queue for MpdBackend<'_> {
    fn add(&mut self, items: &[Item], at: InsertAt, after: AfterAdd) -> Result<()> {
        use crate::mpd::commands::QueuePosition;
        
        // Handle Replace mode - clear first
        if at == InsertAt::Replace {
            self.client.clear()?;
        }
        
        // Get current position for InsertAt::Next
        let current_pos = if at == InsertAt::Next {
            self.client.get_status()?.song
        } else {
            None
        };
        
        // Calculate starting position
        let start_pos: Option<usize> = match at {
            InsertAt::End | InsertAt::Replace => None,
            InsertAt::Next => current_pos.map(|p| p as usize + 1),
            InsertAt::Position(p) => Some(p as usize),
        };
        
        // Add each item
        for (i, item) in items.iter().enumerate() {
            let pos = start_pos.map(|p| QueuePosition::Absolute(p + i));
            
            // MPD uses file paths (URIs), item.id should contain the path
            self.client.add(&item.id, pos)?;
        }
        
        // Handle autoplay
        match after {
            AfterAdd::Nothing => {}
            AfterAdd::PlayFirst => {
                if let Some(pos) = start_pos {
                    self.client.play_pos(pos)?;
                } else if !items.is_empty() {
                    // Added at end, get new queue length and play last added
                    let status = self.client.get_status()?;
                    let play_pos = status.playlistlength.saturating_sub(items.len() as u32);
                    self.client.play_pos(play_pos as usize)?;
                }
            }
            AfterAdd::PlayIndex(idx) => {
                if let Some(pos) = start_pos {
                    self.client.play_pos(pos + idx)?;
                }
            }
        }
        
        Ok(())
    }

    fn remove(&mut self, queue_ids: &[u32]) -> Result<()> {
        for id in queue_ids {
            self.client.delete_id(*id)?;
        }
        Ok(())
    }

    fn list(&mut self) -> Result<Vec<Item>> {
        let songs = self.client.playlist_info()?.unwrap_or_default();
        Ok(songs.into_iter().map(|s| {
            let domain_song: crate::domain::Song = s.into();
            Item::from(&domain_song)
        }).collect())
    }

    fn move_items(&mut self, queue_ids: &[u32], to_position: u32) -> Result<()> {
        use crate::mpd::commands::QueuePosition;
        
        for (i, id) in queue_ids.iter().enumerate() {
            self.client.move_id(*id, QueuePosition::Absolute(to_position as usize + i))?;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.client.clear().map_err(Into::into)
    }

    fn play_id(&mut self, queue_id: u32) -> Result<()> {
        self.client.play_id(queue_id).map_err(Into::into)
    }

    fn set_repeat(&mut self, mode: api::Repeat) -> Result<()> {
        match mode {
            api::Repeat::Off => {
                self.client.repeat(false)?;
                self.client.single(crate::mpd::commands::OnOffOneshot::Off)?;
            }
            api::Repeat::All => {
                self.client.repeat(true)?;
                self.client.single(crate::mpd::commands::OnOffOneshot::Off)?;
            }
            api::Repeat::One => {
                self.client.repeat(true)?;
                self.client.single(crate::mpd::commands::OnOffOneshot::On)?;
            }
        }
        Ok(())
    }

    fn set_shuffle(&mut self, enabled: bool) -> Result<()> {
        self.client.random(enabled).map_err(Into::into)
    }
}

impl api::Discovery for MpdBackend<'_> {
    fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        if query.text.is_empty() {
            return Ok(SearchResults::default());
        }
        
        // Build MPD filter - search in any field
        let filter = vec![Filter::new_with_kind(
            Tag::Any,
            query.text,
            FilterKind::Contains,
        )];
        
        let songs = self.client.search(&filter, false)?;
        let items = songs.into_iter().map(|s| {
            let domain_song: crate::domain::Song = s.into();
            Item::from(&domain_song)
        }).collect();
        
        Ok(SearchResults { items })
    }

    fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        let entries = self.client.lsinfo(if path.is_empty() { None } else { Some(path) })?;
        
        let items = entries.0.into_iter().map(|e| match e {
            LsInfoEntry::Dir(d) => Item {
                id: d.full_path.clone(),
                content_type: ContentType::Directory,
                title: d.name,
                subtitle: None,
                thumbnail: None,
                duration: None,
                queue_id: None,
            },
            LsInfoEntry::File(s) => {
                let domain_song: crate::domain::Song = s.into();
                Item::from(&domain_song)
            }
            LsInfoEntry::Playlist(p) => Item {
                id: p.full_path.clone(),
                content_type: ContentType::Playlist,
                title: p.name,
                subtitle: None,
                thumbnail: None,
                duration: None,
                queue_id: None,
            },
        }).collect();
        
        let parent = if path.is_empty() {
            None
        } else {
            path.rsplit_once('/').map(|(p, _)| p.to_string()).or(Some(String::new()))
        };
        
        Ok(BrowseResult {
            path: path.to_string(),
            items,
            parent,
        })
    }

    fn suggestions(&mut self, _partial: &str) -> Result<Vec<String>> {
        // MPD doesn't have search suggestions
        Ok(vec![])
    }

    fn details(&mut self, item: &Item) -> Result<crate::domain::content::ContentDetails> {
        use crate::domain::content::{
            ContentDetails, AlbumContent, ArtistContent, PlaylistContent,
            ContentRef, Extensions, Stat, Action,
        };
        
        match item.content_type {
            ContentType::Album => {
                // MPD: Album is typically a directory path
                // We can list its contents to get tracks
                let entries = self.client.lsinfo(Some(&item.id))?;
                let tracks: Vec<crate::domain::Song> = entries.0.into_iter()
                    .filter_map(|e| match e {
                        crate::mpd::commands::LsInfoEntry::File(s) => Some(s.into()),
                        _ => None,
                    })
                    .collect();
                
                // Build minimal extensions - just actions
                let extensions = Extensions::builder()
                    .stats(vec![Stat::track_count(tracks.len())])
                    .actions(vec![Action::play(), Action::add_to_queue()])
                    .build();
                
                Ok(ContentDetails::Album(AlbumContent {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    artist: ContentRef::artist("", item.subtitle.clone().unwrap_or_default()),
                    tracks,
                    thumbnail: None,
                    year: None,
                    release_type: None,
                    description: None,
                    extensions,
                }))
            }
            ContentType::Artist => {
                // MPD doesn't have a native artist concept with details
                // Return minimal info with just actions
                let extensions = Extensions::builder()
                    .actions(vec![Action::play(), Action::add_to_queue()])
                    .build();
                
                Ok(ContentDetails::Artist(ArtistContent {
                    id: item.id.clone(),
                    name: item.title.clone(),
                    top_songs: vec![],
                    thumbnail: None,
                    bio: None,
                    extensions,
                }))
            }
            ContentType::Playlist => {
                // MPD playlists - list songs in the playlist
                let songs = self.client.list_playlist_info(&item.id, None)?;
                let tracks: Vec<crate::domain::Song> = songs.into_iter()
                    .map(Into::into)
                    .collect();
                
                // Build minimal extensions
                let extensions = Extensions::builder()
                    .stats(vec![Stat::track_count(tracks.len())])
                    .actions(vec![Action::play(), Action::shuffle(), Action::add_to_queue()])
                    .build();
                
                Ok(ContentDetails::Playlist(PlaylistContent {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    tracks,
                    author: None,
                    thumbnail: None,
                    description: None,
                    track_count: None,
                    duration_text: None,
                    extensions,
                }))
            }
            other => Err(anyhow::anyhow!("Cannot get details for content type: {:?}", other)),
        }
    }

    // resolve() uses default implementation
}

impl api::Volume for MpdBackend<'_> {
    fn get(&mut self) -> Result<u8> {
        let status = self.client.get_status()?;
        Ok(status.volume.0 as u8)
    }

    fn set(&mut self, volume: u8) -> Result<()> {
        use crate::mpd::commands::Volume;
        self.client.set_volume(Volume(volume as u32)).map_err(Into::into)
    }
}

impl api::Backend for MpdBackend<'_> {
    fn name(&self) -> &'static str {
        "MPD"
    }

    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::SavedPlaylists,
            Capability::Stickers,
            Capability::Outputs,
        ]
    }
}
