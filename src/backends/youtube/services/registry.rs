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
//! - `YouTubeMediaPreparerHandle`: Single work queue for cache operations
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
//! │  media_preparer: YouTubeMediaPreparerHandle ←── shared     │
//! └─────────────────────────────────────────────────────┘
//!           │           │           │
//!           ▼           ▼           ▼
//!    ┌──────────┐ ┌──────────┐ ┌──────────┐
//!    │MediaPrep │ │Ffmpeg    │ │Playback  │
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
    audio::{AudioDeliveryPlan, CacheConfig, cache::AudioCache},
    config::{ExtractorType, YtDlpExtractorConfig},
    media::{MediaPreparer, YouTubeMediaPreparer, YouTubeMediaPreparerHandle},
    url_resolver::UrlResolver,
};

#[derive(Clone)]
pub struct YouTubeServices {
    url_resolver: Arc<UrlResolver>,
    audio_cache: Arc<AudioCache>,
    media_preparer: YouTubeMediaPreparerHandle,
}

impl YouTubeServices {
    pub fn new(
        extractor_type: ExtractorType,
        ytdlp_config: YtDlpExtractorConfig,
        audio_source_plan: AudioDeliveryPlan,
        cache_config: CacheConfig,
    ) -> Result<Self> {
        let url_resolver = Arc::new(UrlResolver::with_ytdlp_config(extractor_type, ytdlp_config));
        let audio_cache = Arc::new(AudioCache::new(cache_config)?);
        let media_preparer = YouTubeMediaPreparer::spawn(
            Arc::clone(&url_resolver),
            Arc::clone(&audio_cache),
            audio_source_plan,
        );

        Ok(Self { url_resolver, audio_cache, media_preparer })
    }

    pub fn url_resolver(&self) -> Arc<UrlResolver> {
        Arc::clone(&self.url_resolver)
    }

    pub fn audio_cache(&self) -> Arc<AudioCache> {
        Arc::clone(&self.audio_cache)
    }

    pub fn media_preparer(&self) -> Arc<dyn MediaPreparer> {
        Arc::new(self.media_preparer.clone())
    }

    pub fn shutdown(&self) {
        self.media_preparer.shutdown();
    }
}

impl std::fmt::Debug for YouTubeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YouTubeServices")
            .field("url_resolver", &self.url_resolver)
            .field("audio_cache", &"Arc<AudioCache>")
            .field("media_preparer", &"YouTubeMediaPreparerHandle")
            .finish()
    }
}
