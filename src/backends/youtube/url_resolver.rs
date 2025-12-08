//! URL resolution facade.
//!
//! This module provides a high-level `UrlResolver` facade that composes
//! extractors with caching and fallback behavior.
//!
//! For direct access to extractors, use the `extractor` module.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use super::config::ExtractorType;
use super::extractor::{
    CacheConfig, CachedExtractor, Extractor, FallbackExtractor, YtDlpExtractor, YtxExtractor,
};

/// URL resolver facade.
///
/// Provides a simple interface for resolving YouTube video IDs to stream URLs:
/// - Configurable primary extractor (ytx or yt-dlp)
/// - Optional fallback to yt-dlp if primary fails
/// - LRU + TTL caching
///
/// # Example
///
/// ```rust,ignore
/// let resolver = UrlResolver::new(ExtractorType::Ytx);
///
/// // Single resolution
/// let url = resolver.get_url("dQw4w9WgXcQ")?;
///
/// // Batch resolution (efficient for queue prefetch)
/// let urls = resolver.get_urls(&["id1", "id2", "id3"]);
/// ```
#[derive(Clone)]
pub struct UrlResolver {
    inner: Arc<dyn Extractor>,
    extractor_type: ExtractorType,
}

impl std::fmt::Debug for UrlResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlResolver")
            .field("extractor_type", &self.extractor_type)
            .field("inner", &self.inner.name())
            .finish()
    }
}

impl UrlResolver {
    /// Create a new extractor with the specified type.
    ///
    /// The extractor is wrapped with caching and fallback automatically.
    pub fn new(extractor_type: ExtractorType) -> Self {
        Self::with_config(extractor_type, CacheConfig::default(), true)
    }

    /// Create a new extractor with custom configuration.
    ///
    /// # Arguments
    /// * `extractor_type` - Primary extractor to use
    /// * `cache_config` - Cache configuration
    /// * `enable_fallback` - Whether to fall back to yt-dlp if primary fails
    pub fn with_config(
        extractor_type: ExtractorType,
        cache_config: CacheConfig,
        enable_fallback: bool,
    ) -> Self {
        let inner: Arc<dyn Extractor> = match extractor_type {
            ExtractorType::Ytx => {
                if enable_fallback {
                    Arc::new(CachedExtractor::with_config(
                        FallbackExtractor::new(YtxExtractor::new(), YtDlpExtractor::new()),
                        cache_config,
                    ))
                } else {
                    Arc::new(CachedExtractor::with_config(YtxExtractor::new(), cache_config))
                }
            }
            ExtractorType::YtDlp => {
                // yt-dlp is the fallback, so no fallback needed
                Arc::new(CachedExtractor::with_config(
                    YtDlpExtractor::new(),
                    cache_config,
                ))
            }
        };

        Self {
            inner,
            extractor_type,
        }
    }

    /// Create with default extractor (yt-dlp).
    pub fn new_default() -> Self {
        Self::new(ExtractorType::default())
    }

    /// Create with custom cache TTL.
    pub fn with_cache_ttl(extractor_type: ExtractorType, ttl: Duration) -> Self {
        Self::with_config(
            extractor_type,
            CacheConfig::default().with_ttl(ttl),
            true,
        )
    }

    /// Get the configured extractor type.
    pub fn extractor_type(&self) -> ExtractorType {
        self.extractor_type
    }

    /// Get stream URL for a video ID, using cache if available.
    pub fn get_url(&self, video_id: &str) -> Result<String> {
        self.inner.extract_one(video_id)
    }

    /// Get stream URLs for multiple video IDs.
    ///
    /// Efficient for queue prefetching - only uncached IDs are extracted.
    /// Returns a map of video_id → Result<url>.
    pub fn get_urls(&self, video_ids: &[String]) -> std::collections::HashMap<String, Result<String>> {
        self.inner.extract_batch(video_ids)
    }

    /// Prefetch URLs for multiple video IDs in background.
    ///
    /// Results are stored in cache for later retrieval.
    pub fn prefetch(&self, video_ids: Vec<String>) {
        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            let results = inner.extract_batch(&video_ids);
            // Log failures
            for (id, result) in &results {
                if let Err(e) = result {
                    log::warn!("Failed to prefetch {}: {}", id, e);
                }
            }
        });
    }

    /// Clear the URL cache.
    pub fn clear_cache(&self) {
        self.inner.clear_cache();
    }
}

impl Default for UrlResolver {
    fn default() -> Self {
        Self::new_default()
    }
}

// Make UrlResolver usable as an Extractor trait object
impl Extractor for UrlResolver {
    fn extract_batch(&self, video_ids: &[String]) -> std::collections::HashMap<String, Result<String>> {
        self.inner.extract_batch(video_ids)
    }

    fn name(&self) -> &'static str {
        match self.extractor_type {
            ExtractorType::Ytx => "ytx",
            ExtractorType::YtDlp => "yt-dlp",
        }
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        self.inner.extract_one(video_id)
    }

    fn clear_cache(&self) {
        self.inner.clear_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extractor_type() {
        let ytdlp = UrlResolver::new(ExtractorType::YtDlp);
        assert_eq!(ytdlp.extractor_type(), ExtractorType::YtDlp);

        let ytx = UrlResolver::new(ExtractorType::Ytx);
        assert_eq!(ytx.extractor_type(), ExtractorType::Ytx);
    }

    #[test]
    fn test_default_is_ytdlp() {
        let extractor = UrlResolver::default();
        assert_eq!(extractor.extractor_type(), ExtractorType::YtDlp);
    }
}
