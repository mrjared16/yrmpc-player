//! URL resolution facade.
//!
//! This module provides a high-level `UrlResolver` facade that composes
//! extractors with caching and fallback behavior.
//!
//! For direct access to extractors, use the `extractor` module.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::{
    config::{DEFAULT_ENABLE_EXTRACTOR_FALLBACK, ExtractorType, YtDlpExtractorConfig},
    extractor::{
        CacheConfig, CachedExtractor, Extractor, FallbackExtractor, YtDlpExtractor, YtxExtractor,
    },
    media::{AudioFormat, StreamInfo, StreamResolver},
};

#[derive(Debug, Clone)]
pub struct UrlStreamInfo {
    pub url: String,
    pub content_length: u64,
    pub bitrate: u32,
    pub mime_type: String,
}

/// URL resolver facade.
///
/// Provides a simple interface for resolving YouTube video IDs to stream URLs:
/// - Configurable primary extractor (ytx or yt-dlp)
/// - Optional fallback to the other extractor if primary fails
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
    ///
    /// # Visibility
    ///
    /// This is `pub(crate)` to enforce service sharing via `YouTubeServices`.
    /// External code should obtain resolvers from
    /// `YouTubeServices::url_resolver()`.
    pub(crate) fn new(extractor_type: ExtractorType) -> Self {
        Self::with_ytdlp_config(
            extractor_type,
            DEFAULT_ENABLE_EXTRACTOR_FALLBACK,
            YtDlpExtractorConfig::default(),
        )
    }

    pub(crate) fn with_ytdlp_config(
        extractor_type: ExtractorType,
        enable_fallback: bool,
        ytdlp_config: YtDlpExtractorConfig,
    ) -> Self {
        Self::with_config(extractor_type, CacheConfig::default(), enable_fallback, ytdlp_config)
    }

    /// Create a new extractor with custom configuration.
    ///
    /// # Visibility
    ///
    /// This is `pub(crate)` to enforce service sharing via `YouTubeServices`.
    ///
    /// # Arguments
    /// * `extractor_type` - Primary extractor to use
    /// * `cache_config` - Cache configuration
    /// * `enable_fallback` - Whether to try the other extractor if primary fails
    pub(crate) fn with_config(
        extractor_type: ExtractorType,
        cache_config: CacheConfig,
        enable_fallback: bool,
        ytdlp_config: YtDlpExtractorConfig,
    ) -> Self {
        let cookies_path = ytdlp_config.cookies_path;
        let ytx_extractor =
            || cookies_path.clone().map(YtxExtractor::with_cookies).unwrap_or_default();
        let ytdlp_extractor = || YtDlpExtractor::with_options(cookies_path.clone());

        let inner: Arc<dyn Extractor> = match (extractor_type, enable_fallback) {
            (ExtractorType::Ytx, true) => Arc::new(CachedExtractor::with_config(
                FallbackExtractor::new(ytx_extractor(), ytdlp_extractor()),
                cache_config,
            )),
            (ExtractorType::Ytx, false) => {
                Arc::new(CachedExtractor::with_config(ytx_extractor(), cache_config))
            }
            (ExtractorType::YtDlp, true) => {
                let primary = ytdlp_extractor();
                primary.eager_bootstrap_po_token_provider();
                Arc::new(CachedExtractor::with_config(
                    FallbackExtractor::new(primary, ytx_extractor()),
                    cache_config,
                ))
            }
            (ExtractorType::YtDlp, false) => {
                let primary = ytdlp_extractor();
                primary.eager_bootstrap_po_token_provider();
                Arc::new(CachedExtractor::with_config(primary, cache_config))
            }
        };

        Self { inner, extractor_type }
    }

    /// Create with default extractor type.
    ///
    /// # Visibility
    ///
    /// This is `pub(crate)` to enforce service sharing via `YouTubeServices`.
    pub(crate) fn new_default() -> Self {
        Self::new(ExtractorType::default())
    }

    #[cfg(test)]
    pub(crate) fn from_extractor(inner: Arc<dyn Extractor>, extractor_type: ExtractorType) -> Self {
        Self { inner, extractor_type }
    }

    #[cfg(test)]
    pub(crate) fn from_cached_extractor<E: Extractor + 'static>(
        inner: E,
        extractor_type: ExtractorType,
    ) -> Self {
        Self { inner: Arc::new(CachedExtractor::new(inner)), extractor_type }
    }

    /// Create with custom cache TTL.
    ///
    /// # Visibility
    ///
    /// This is `pub(crate)` to enforce service sharing via `YouTubeServices`.
    pub(crate) fn with_cache_ttl(extractor_type: ExtractorType, ttl: Duration) -> Self {
        Self::with_config(
            extractor_type,
            CacheConfig::default().with_ttl(ttl),
            DEFAULT_ENABLE_EXTRACTOR_FALLBACK,
            YtDlpExtractorConfig::default(),
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

    pub fn get_url_fresh(&self, video_id: &str) -> Result<String> {
        self.inner.extract_one_fresh(video_id)
    }

    pub fn get_stream_info(&self, video_id: &str) -> Result<UrlStreamInfo> {
        let url = self.get_url(video_id)?;
        let (content_length, mime_type) = Self::probe_stream_headers(&url)?;
        Ok(UrlStreamInfo { url, content_length, bitrate: 0, mime_type })
    }

    #[allow(dead_code)]
    pub fn get_stream_infos(
        &self,
        video_ids: &[String],
    ) -> std::collections::HashMap<String, Result<UrlStreamInfo>> {
        let urls = self.get_urls(video_ids);
        urls.into_iter()
            .map(|(video_id, url_result)| {
                let info_result = url_result.and_then(|url| {
                    let (content_length, mime_type) = Self::probe_stream_headers(&url)?;
                    Ok(UrlStreamInfo { url, content_length, bitrate: 0, mime_type })
                });
                (video_id, info_result)
            })
            .collect()
    }

    #[allow(dead_code)]
    fn probe_stream_headers(url: &str) -> Result<(u64, String)> {
        use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};

        log::debug!("[PROBE] starting HEAD request");
        let client = reqwest::blocking::Client::new();

        if let Ok(resp) = client.head(url).send() {
            if resp.status().is_success() {
                let content_length = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| resp.content_length())
                    .unwrap_or(0);

                let mime_type = resp
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                if content_length > 0 {
                    return Ok((content_length, mime_type));
                }
            }
        }

        let resp = client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .context("Failed to probe stream headers")?;

        let mime_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if let Some(range) = resp.headers().get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
            if let Some((_, total)) = range.split_once('/') {
                if let Ok(total) = total.parse::<u64>() {
                    return Ok((total, mime_type));
                }
            }
        }

        let content_length = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        Ok((content_length, mime_type))
    }

    /// Get stream URLs for multiple video IDs.
    ///
    /// Efficient for queue prefetching - only uncached IDs are extracted.
    /// Returns a map of video_id → Result<url>.
    pub fn get_urls(
        &self,
        video_ids: &[String],
    ) -> std::collections::HashMap<String, Result<String>> {
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

    pub fn invalidate(&self, video_id: &str) {
        self.inner.invalidate(video_id);
    }

    /// Re-extract a fresh URL, invalidating only the cache (not dedup).
    ///
    /// Used by relay 403 retry to avoid cascading yt-dlp extractions.
    pub fn refresh_url(&self, video_id: &str) -> Result<String> {
        self.inner.refresh(video_id)
    }
}

#[async_trait]
impl StreamResolver for UrlResolver {
    async fn resolve(&self, track_id: &str) -> Result<StreamInfo> {
        let url = self.get_url(track_id)?;
        Ok(StreamInfo {
            url,
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            format: AudioFormat::default(),
        })
    }

    fn is_cached(&self, track_id: &str) -> bool {
        self.inner.is_cached(track_id)
    }

    fn invalidate(&self, track_id: &str) {
        self.inner.invalidate(track_id)
    }
}

impl Default for UrlResolver {
    fn default() -> Self {
        Self::new_default()
    }
}

// Make UrlResolver usable as an Extractor trait object
impl Extractor for UrlResolver {
    fn extract_batch(
        &self,
        video_ids: &[String],
    ) -> std::collections::HashMap<String, Result<String>> {
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

    fn is_cached(&self, video_id: &str) -> bool {
        self.inner.is_cached(video_id)
    }

    fn invalidate(&self, video_id: &str) {
        self.inner.invalidate(video_id);
    }

    fn refresh(&self, video_id: &str) -> Result<String> {
        self.inner.refresh(video_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::youtube::extractor::YtDlpExtractor;

    #[test]
    fn test_extractor_type() {
        let ytdlp = UrlResolver::new(ExtractorType::YtDlp);
        assert_eq!(ytdlp.extractor_type(), ExtractorType::YtDlp);

        let ytx = UrlResolver::new(ExtractorType::Ytx);
        assert_eq!(ytx.extractor_type(), ExtractorType::Ytx);
    }

    #[test]
    fn test_default_is_ytx() {
        let extractor = UrlResolver::default();
        assert_eq!(extractor.extractor_type(), ExtractorType::Ytx);
    }

    #[test]
    fn test_ytdlp_path_attempts_eager_bootstrap() {
        YtDlpExtractor::reset_eager_bootstrap_attempts_for_tests();

        let _resolver = UrlResolver::with_config(
            ExtractorType::YtDlp,
            CacheConfig::default(),
            false,
            YtDlpExtractorConfig::default(),
        );

        assert_eq!(YtDlpExtractor::eager_bootstrap_attempts_for_tests(), 1);
    }

    #[test]
    fn test_ytx_path_does_not_attempt_eager_bootstrap() {
        YtDlpExtractor::reset_eager_bootstrap_attempts_for_tests();

        let _resolver = UrlResolver::with_config(
            ExtractorType::Ytx,
            CacheConfig::default(),
            false,
            YtDlpExtractorConfig::default(),
        );

        assert_eq!(YtDlpExtractor::eager_bootstrap_attempts_for_tests(), 0);
    }

    #[test]
    fn test_ytx_primary_with_fallback_does_not_attempt_eager_bootstrap() {
        YtDlpExtractor::reset_eager_bootstrap_attempts_for_tests();

        let _resolver = UrlResolver::with_config(
            ExtractorType::Ytx,
            CacheConfig::default(),
            true,
            YtDlpExtractorConfig::default(),
        );

        assert_eq!(YtDlpExtractor::eager_bootstrap_attempts_for_tests(), 0);
    }

    #[test]
    fn test_ytdlp_primary_with_fallback_attempts_eager_bootstrap() {
        YtDlpExtractor::reset_eager_bootstrap_attempts_for_tests();

        let _resolver = UrlResolver::with_config(
            ExtractorType::YtDlp,
            CacheConfig::default(),
            true,
            YtDlpExtractorConfig::default(),
        );

        assert_eq!(YtDlpExtractor::eager_bootstrap_attempts_for_tests(), 1);
    }
}
