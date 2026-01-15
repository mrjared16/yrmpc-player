//! Service Registry - Single source of truth for shared services.
//!
//! This module implements the Service Locator pattern to ensure
//! stateful services are instantiated ONCE and shared across all components.
//!
//! # Why This Exists
//!
//! The YouTube backend has several stateful services that MUST be shared:
//! - `UrlResolver`: Has an extraction cache (prevents duplicate yt-dlp calls)
//! - `AudioCache`: Manages downloaded audio files
//! - `CacheExecutorHandle`: Single work queue for cache operations
//!
//! Previously, components created their own instances, causing bugs like
//! duplicate URL extractions (each resolver had its own empty cache).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                  YouTubeServices                    │
//! │  (created ONCE at server startup)                   │
//! ├─────────────────────────────────────────────────────┤
//! │  url_resolver: Arc<UrlResolver>     ←── shared      │
//! │  audio_cache: Arc<AudioCache>       ←── shared      │
//! │  cache_executor: CacheExecutorHandle ←── shared     │
//! └─────────────────────────────────────────────────────┘
//!           │           │           │
//!           ▼           ▼           ▼
//!    ┌──────────┐ ┌──────────┐ ┌──────────┐
//!    │CacheExec │ │Ffmpeg    │ │Playback  │
//!    │          │ │ConcatSrc │ │Service   │
//!    └──────────┘ └──────────┘ └──────────┘
//! ```
//!
//! # Usage
//!
//! Components receive `&YouTubeServices` and extract what they need:
//!
//! ```rust,ignore
//! fn new(services: &YouTubeServices) -> Self {
//!     Self {
//!         resolver: services.url_resolver(),
//!         cache: services.audio_cache(),
//!     }
//! }
//! ```

use std::sync::Arc;

use anyhow::Result;

use crate::backends::youtube::{
    audio::{cache::AudioCache, CacheConfig},
    config::ExtractorType,
    url_resolver::UrlResolver,
};

use super::{CacheExecutor, CacheExecutorHandle};

#[derive(Clone)]
pub struct YouTubeServices {
    url_resolver: Arc<UrlResolver>,
    audio_cache: Arc<AudioCache>,
    cache_executor: CacheExecutorHandle,
}

impl YouTubeServices {
    pub fn new(extractor_type: ExtractorType, cache_config: CacheConfig) -> Result<Self> {
        let url_resolver = Arc::new(UrlResolver::new(extractor_type));
        let audio_cache = Arc::new(AudioCache::new(cache_config)?);
        let cache_executor =
            CacheExecutor::spawn(Arc::clone(&url_resolver), Arc::clone(&audio_cache));

        Ok(Self { url_resolver, audio_cache, cache_executor })
    }

    pub fn url_resolver(&self) -> Arc<UrlResolver> {
        Arc::clone(&self.url_resolver)
    }

    pub fn audio_cache(&self) -> Arc<AudioCache> {
        Arc::clone(&self.audio_cache)
    }

    pub fn cache_executor(&self) -> CacheExecutorHandle {
        self.cache_executor.clone()
    }

    pub fn shutdown(&self) {
        self.cache_executor.shutdown();
    }
}

impl std::fmt::Debug for YouTubeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YouTubeServices")
            .field("url_resolver", &self.url_resolver)
            .field("audio_cache", &"Arc<AudioCache>")
            .field("cache_executor", &"CacheExecutorHandle")
            .finish()
    }
}
