use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Result;

use crate::backends::api::{
    self, AfterAdd, Backend, BrowseResult, Capability, ContentDetails, Discovery, InsertAt, Item,
    Playback, Queue, Repeat, SearchQuery, SearchResults, Status, StatusQuery, Volume,
};

#[derive(Debug, Clone)]
pub struct MockBackend {
    pub status: Arc<RwLock<Status>>,
    pub queue: Arc<RwLock<Vec<Item>>>,
    pub volume: Arc<RwLock<u8>>,
    pub search_results: Arc<RwLock<SearchResults>>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(Status {
                state: api::State::Stopped,
                position: Some(Duration::ZERO),
                duration: None,
                volume: 100,
                repeat: Repeat::Off,
                shuffle: false,
                crossfade: 0,
                gapless: false,
            })),
            queue: Arc::new(RwLock::new(Vec::new())),
            volume: Arc::new(RwLock::new(100)),
            search_results: Arc::new(RwLock::new(SearchResults::default())),
        }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        "Mock"
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::RichMetadata]
    }
}

impl Playback for MockBackend {
    fn play(&mut self) -> Result<()> {
        self.status.write().unwrap().state = api::State::Playing;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.status.write().unwrap().state = api::State::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.status.write().unwrap().state = api::State::Stopped;
        Ok(())
    }

    fn next(&mut self) -> Result<()> {
        Ok(())
    }

    fn previous(&mut self) -> Result<()> {
        Ok(())
    }

    fn seek(&mut self, position: Duration) -> Result<()> {
        self.status.write().unwrap().position = Some(position);
        Ok(())
    }

    fn seek_relative(&mut self, delta_secs: i64) -> Result<()> {
        let mut status = self.status.write().unwrap();
        let current = status.position.unwrap_or_default().as_secs() as i64;
        let new_secs = (current + delta_secs).max(0) as u64;
        status.position = Some(Duration::from_secs(new_secs));
        Ok(())
    }

    fn status(&mut self) -> Result<Status> {
        Ok(self.status.read().unwrap().clone())
    }
}

impl Queue for MockBackend {
    fn add(&mut self, items: &[Item], at: InsertAt, _after: AfterAdd) -> Result<()> {
        let mut queue = self.queue.write().unwrap();
        match at {
            InsertAt::End => queue.extend_from_slice(items),
            InsertAt::Next => {
                for item in items.iter().rev() {
                    queue.insert(0, item.clone());
                }
            }
            InsertAt::Position(pos) => {
                let pos = (pos as usize).min(queue.len());
                for (i, item) in items.iter().enumerate() {
                    queue.insert(pos + i, item.clone());
                }
            }
            InsertAt::Replace => {
                queue.clear();
                queue.extend_from_slice(items);
            }
        }
        Ok(())
    }

    fn remove(&mut self, _queue_ids: &[u32]) -> Result<()> {
        Ok(())
    }

    fn list(&mut self) -> Result<Vec<Item>> {
        Ok(self.queue.read().unwrap().clone())
    }

    fn move_items(&mut self, _queue_ids: &[u32], _to_position: u32) -> Result<()> {
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.queue.write().unwrap().clear();
        Ok(())
    }

    fn play_id(&mut self, _queue_id: u32) -> Result<()> {
        Ok(())
    }

    fn set_repeat(&mut self, mode: Repeat) -> Result<()> {
        self.status.write().unwrap().repeat = mode;
        Ok(())
    }

    fn set_shuffle(&mut self, enabled: bool) -> Result<()> {
        self.status.write().unwrap().shuffle = enabled;
        Ok(())
    }
}

impl Discovery for MockBackend {
    fn search(&mut self, _query: SearchQuery) -> Result<SearchResults> {
        Ok(self.search_results.read().unwrap().clone())
    }

    fn browse(&mut self, path: &str) -> Result<BrowseResult> {
        Ok(BrowseResult { path: path.to_string(), items: vec![], parent: None })
    }

    fn suggestions(&mut self, _partial: &str) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn resolve(&mut self, item: &Item) -> Result<Vec<Item>> {
        Ok(vec![item.clone()])
    }

    fn details(&mut self, _item: &Item) -> Result<ContentDetails> {
        // Return dummy details
        use crate::domain::content::{AlbumContent, ContentDetails, ContentRef};
        Ok(ContentDetails::Album(AlbumContent {
            id: "mock_id".into(),
            title: "Mock Album".into(),
            artist: ContentRef::new("mock_artist", "Mock Artist"),
            tracks: vec![],
            thumbnail: None,
            year: None,
            release_type: None,
            description: None,
            extensions: crate::domain::content::Extensions::builder().build(),
        }))
    }
}

impl Volume for MockBackend {
    fn get(&mut self) -> Result<u8> {
        Ok(*self.volume.read().unwrap())
    }

    fn set(&mut self, volume: u8) -> Result<()> {
        *self.volume.write().unwrap() = volume;
        Ok(())
    }
}

impl StatusQuery for MockBackend {
    fn get_status(&mut self) -> Result<crate::domain::Status> {
        let status = self.status.read().unwrap();
        Ok(crate::domain::Status {
            state: match status.state {
                api::State::Playing => crate::domain::PlaybackState::Play,
                api::State::Paused => crate::domain::PlaybackState::Pause,
                api::State::Stopped => crate::domain::PlaybackState::Stop,
            },
            volume: status.volume,
            elapsed: status.position,
            duration: status.duration,
            repeat: matches!(status.repeat, Repeat::All | Repeat::One),
            random: status.shuffle,
            single: if matches!(status.repeat, Repeat::One) {
                crate::domain::status::OnOffOneshot::On
            } else {
                crate::domain::status::OnOffOneshot::Off
            },
            consume: crate::domain::status::OnOffOneshot::Off,
            playlistlength: self.queue.read().unwrap().len() as u32,
            playlist: Some(1),
            songid: None,
            song_position: None,
            next_songid: None,
            next_song_position: None,
            bitrate: None,
            xfade: None,
            updating_db: None,
            error: None,
            partition: "default".into(),
            lastloadedplaylist: None,
        })
    }

    fn current_song(&mut self) -> Result<Option<crate::domain::Song>> {
        Ok(None)
    }

    fn queue_songs(&mut self) -> Result<Vec<crate::domain::Song>> {
        Ok(vec![])
    }
}
