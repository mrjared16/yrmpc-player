//! Stream URL extraction with composable extractors.
//!
//! This module provides a trait-based extraction system with decorators
//! for caching, fallback, and other behaviors.
//!
//! # Example
//!
//! ```rust,ignore
//! let extractor = CachedExtractor::new(
//!     FallbackExtractor::new(
//!         YtxExtractor::new(),
//!         YtDlpExtractor::new(),
//!     ),
//!     CacheConfig::default(),
//! );
//!
//! // Single extraction (hits cache if available)
//! let url = extractor.extract_one("dQw4w9WgXcQ")?;
//!
//! // Batch extraction (efficient for queue prefetch)
//! let results = extractor.extract_batch(&["id1", "id2", "id3"]);
//! ```

mod cached;
mod fallback;
mod ytdlp;
mod ytx;

use std::collections::HashMap;

use anyhow::{Result, anyhow};
pub use cached::{CacheConfig, CachedExtractor};
pub use fallback::FallbackExtractor;
pub use ytdlp::YtDlpExtractor;
pub use ytx::YtxExtractor;

/// Core extraction trait - all extractors implement this.
///
/// Extractors can be composed using decorators like `CachedExtractor`
/// and `FallbackExtractor` for additional behaviors.
pub trait Extractor: Send + Sync {
    /// Extract URLs for multiple video IDs.
    ///
    /// Returns a map of video_id → Result<url>. Partial failures are
    /// expected (some IDs may fail while others succeed).
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>>;

    /// Extractor name for logging and metrics.
    fn name(&self) -> &'static str;

    /// Extract URL for a single video ID.
    ///
    /// Default implementation delegates to `extract_batch` with a single ID.
    fn extract_one(&self, video_id: &str) -> Result<String> {
        let id = video_id.to_string();
        self.extract_batch(&[id.clone()])
            .remove(&id)
            .unwrap_or_else(|| Err(anyhow!("No result returned for {}", video_id)))
    }

    /// Clear any cached data.
    fn clear_cache(&self);

    /// Check if a video ID is in the cache.
    fn is_cached(&self, video_id: &str) -> bool;

    /// Invalidate (remove) a specific video ID from the cache.
    fn invalidate(&self, video_id: &str);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock extractor for testing
    struct MockExtractor {
        responses: HashMap<String, Result<String>>,
    }

    impl MockExtractor {
        fn new(responses: Vec<(&str, Result<String>)>) -> Self {
            Self { responses: responses.into_iter().map(|(k, v)| (k.to_string(), v)).collect() }
        }
    }

    impl Extractor for MockExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids
                .iter()
                .filter_map(|id| {
                    self.responses.get(id).map(|r| {
                        (id.clone(), r.as_ref().map(|s| s.clone()).map_err(|e| anyhow!("{}", e)))
                    })
                })
                .collect()
        }

        fn name(&self) -> &'static str {
            "mock"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    #[test]
    fn test_extract_one_delegates_to_batch() {
        let extractor =
            MockExtractor::new(vec![("abc123", Ok("https://example.com/stream".to_string()))]);

        let url = extractor.extract_one("abc123").unwrap();
        assert_eq!(url, "https://example.com/stream");
    }

    #[test]
    fn test_extract_one_missing_id() {
        let extractor = MockExtractor::new(vec![]);
        let result = extractor.extract_one("missing");
        assert!(result.is_err());
    }
}
