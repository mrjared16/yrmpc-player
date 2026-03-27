use std::{
    collections::HashSet,
    io::Cursor,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use crossbeam::channel::Sender;
use image::{DynamicImage, ImageReader};
use lru::LruCache;
use ratatui::layout::Rect;
use ratatui_image::{Resize, picker::Picker, protocol::Protocol};

use crate::AppEvent;

/// Standard thumbnail sizes for different UI contexts.
/// Using an enum instead of arbitrary (width, height) to:
/// 1. Bound cache size (max 3 Protocols per image)
/// 2. Enable pre-creation of most common size
/// 3. Avoid cache key explosion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbnailSize {
    /// List items: 4×2 cells (most common, pre-created on fetch)
    ListItem,
    /// Preview panel: larger thumbnail
    Preview,
    /// Full album art display
    AlbumArt,
}

impl ThumbnailSize {
    /// Get the Rect dimensions for this thumbnail size
    pub fn to_rect(self) -> Rect {
        match self {
            ThumbnailSize::ListItem => Rect::new(0, 0, 6, 3),
            ThumbnailSize::Preview => Rect::new(0, 0, 12, 6),
            ThumbnailSize::AlbumArt => Rect::new(0, 0, 24, 12),
        }
    }
}

/// Cache key for Protocol lookup
type ProtocolKey = (String, ThumbnailSize);

/// Inner cache state protected by single mutex
#[derive(Debug)]
struct CacheInner {
    /// Raw images (source of truth)
    raw: LruCache<String, Arc<DynamicImage>>,
    /// Protocols by (url, size) - derived artifacts
    protocols: LruCache<ProtocolKey, Arc<Mutex<Protocol>>>,
}

#[derive(Clone, Debug)]
pub struct ImageCache {
    inner: Arc<Mutex<CacheInner>>,
    pending_fetch: Arc<Mutex<HashSet<String>>>,
    pending_protocol: Arc<Mutex<HashSet<ProtocolKey>>>,
    picker: Picker,
    app_event_sender: Sender<AppEvent>,
}

impl ImageCache {
    pub(crate) fn new(app_event_sender: Sender<AppEvent>) -> Self {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));

        let inner = CacheInner {
            raw: LruCache::new(NonZeroUsize::new(50).unwrap()),
            protocols: LruCache::new(NonZeroUsize::new(100).unwrap()),
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
            pending_fetch: Arc::new(Mutex::new(HashSet::new())),
            pending_protocol: Arc::new(Mutex::new(HashSet::new())),
            picker,
            app_event_sender,
        }
    }

    /// Get a Protocol for the given URL and size.
    /// Returns cached Protocol if available, otherwise triggers async creation.
    ///
    /// This is the main API - called from AsyncImage at render time.
    pub fn get_protocol(&self, url: &str, size: ThumbnailSize) -> Option<Arc<Mutex<Protocol>>> {
        let key = (url.to_string(), size);

        {
            let mut inner = self.inner.lock().unwrap();

            // 1. Check Protocol cache (fast path)
            if let Some(protocol) = inner.protocols.get(&key) {
                return Some(protocol.clone());
            }

            // 2. If raw image exists, spawn async Protocol creation
            if inner.raw.contains(&url.to_string()) {
                drop(inner); // Release lock
                self.spawn_protocol_creation(url.to_string(), size);
                return None;
            }
        }

        // 3. No raw image - start fetch if not already pending
        self.spawn_fetch(url.to_string());

        None
    }

    /// Legacy API for backward compatibility - uses ListItem size
    pub fn get(&self, url: &str) -> Option<Arc<Mutex<Protocol>>> {
        self.get_protocol(url, ThumbnailSize::ListItem)
    }

    /// Spawn async image fetch
    fn spawn_fetch(&self, url: String) {
        {
            let mut pending = self.pending_fetch.lock().unwrap();
            if pending.contains(&url) {
                return;
            }
            pending.insert(url.clone());
        }

        let sender = self.app_event_sender.clone();
        let inner = self.inner.clone();
        let picker = self.picker.clone();
        let pending_fetch = self.pending_fetch.clone();

        std::thread::spawn(move || {
            let result = fetch_image_sync(&url);

            match result {
                Ok(img) => {
                    let img = Arc::new(img);

                    // Pre-create Protocol for ListItem (most common size)
                    let list_protocol = picker.new_protocol(
                        img.as_ref().clone(),
                        ThumbnailSize::ListItem.to_rect(),
                        Resize::Fit(None),
                    );

                    {
                        let mut inner = inner.lock().unwrap();
                        inner.raw.put(url.clone(), img.clone());

                        // Store pre-created ListItem Protocol
                        if let Ok(protocol) = list_protocol {
                            let key = (url.clone(), ThumbnailSize::ListItem);
                            inner.protocols.put(key, Arc::new(Mutex::new(protocol)));
                        }
                    }

                    // Notify UI to redraw
                    let _ = sender.send(AppEvent::UiEvent(crate::ui::UiAppEvent::Redraw));
                }
                Err(e) => {
                    log::warn!("Failed to fetch image {url}: {e}");
                }
            }

            let mut pending = pending_fetch.lock().unwrap();
            pending.remove(&url);
        });
    }

    /// Spawn async Protocol creation for a size other than ListItem
    fn spawn_protocol_creation(&self, url: String, size: ThumbnailSize) {
        let key = (url.clone(), size);
        {
            let mut pending = self.pending_protocol.lock().unwrap();
            if pending.contains(&key) {
                return;
            }
            pending.insert(key.clone());
        }

        let sender = self.app_event_sender.clone();
        let inner = self.inner.clone();
        let picker = self.picker.clone();
        let pending_protocol = self.pending_protocol.clone();

        std::thread::spawn(move || {
            // Get raw image from cache
            let img = {
                let inner = inner.lock().unwrap();
                inner.raw.peek(&url).cloned()
            };

            if let Some(img) = img {
                let protocol_result =
                    picker.new_protocol(img.as_ref().clone(), size.to_rect(), Resize::Fit(None));

                if let Ok(protocol) = protocol_result {
                    let p = Arc::new(Mutex::new(protocol));
                    {
                        let mut inner = inner.lock().unwrap();
                        inner.protocols.put(key.clone(), p);
                    }
                    let _ = sender.send(AppEvent::UiEvent(crate::ui::UiAppEvent::Redraw));
                }
            }

            let mut pending = pending_protocol.lock().unwrap();
            pending.remove(&key);
        });
    }
}

/// Fetch and decode image synchronously (called from background thread)
fn fetch_image_sync(url: &str) -> Result<DynamicImage> {
    let bytes = reqwest::blocking::get(url)?.bytes()?;
    let img = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.decode()?;
    Ok(img)
}
