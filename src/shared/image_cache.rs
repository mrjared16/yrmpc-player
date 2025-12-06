use std::{
    collections::{HashSet},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use crossbeam::channel::Sender;
use image::ImageReader;
use lru::LruCache;
use ratatui_image::{
    picker::{Picker},
    protocol::Protocol,
};
use std::io::Cursor;
use std::num::NonZeroUsize;

use crate::AppEvent;

#[derive(Clone, Debug)]
pub struct ImageCache {
    cache: Arc<Mutex<LruCache<String, Arc<Mutex<Protocol>>>>>,
    pending: Arc<Mutex<HashSet<String>>>,
    picker: Picker,
    app_event_sender: Sender<AppEvent>,
}

impl ImageCache {
    pub fn new(app_event_sender: Sender<AppEvent>) -> Self {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
        let cache = LruCache::new(NonZeroUsize::new(50).unwrap());

        Self {
            cache: Arc::new(Mutex::new(cache)),
            pending: Arc::new(Mutex::new(HashSet::new())),
            picker,
            app_event_sender,
        }
    }
// ...

    pub fn get(&self, url: &str) -> Option<Arc<Mutex<Protocol>>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(img) = cache.get(url) {
            return Some(img.clone());
        }

        // If not in cache, check if pending
        let mut pending = self.pending.lock().unwrap();
        if !pending.contains(url) {
            pending.insert(url.to_string());
            self.spawn_fetch(url.to_string());
        }

        None
    }

    fn spawn_fetch(&self, url: String) {
        let sender = self.app_event_sender.clone();
        let cache = self.cache.clone();
        let pending = self.pending.clone();
        let picker = self.picker.clone();

        std::thread::spawn(move || {
            let result = fetch_and_process_sync(&url, picker);

            let mut pending = pending.lock().unwrap();
            pending.remove(&url);

            match result {
                Ok(protocol) => {
                    let mut cache = cache.lock().unwrap();
                    cache.put(url, protocol);
                    // Notify UI to redraw
                    let _ = sender.send(AppEvent::UiEvent(crate::ui::UiAppEvent::Redraw));
                }
                Err(e) => {
                    log::warn!("Failed to fetch image {}: {}", url, e);
                }
            }
        });
    }
}

fn fetch_and_process_sync(url: &str, picker: Picker) -> Result<Arc<Mutex<Protocol>>> {
    let bytes = reqwest::blocking::get(url)?.bytes()?;
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;
    
    // Resize logic could go here if needed, but ratatui-image handles resizing well
    
    let protocol = picker.new_protocol(img, ratatui::layout::Rect::new(0, 0, 100, 100), ratatui_image::Resize::Fit(None))?;
    Ok(Arc::new(Mutex::new(protocol)))
}
