//! Fallback extractor decorator - tries primary, falls back on failure.
//!
//! Useful for reliability: try fast extractor first, fall back to reliable one.

use std::collections::HashMap;

use anyhow::Result;

use super::Extractor;

/// Fallback extractor decorator.
///
/// Tries the primary extractor first. If it fails, tries the fallback.
/// For batch extraction, only failed IDs are retried with the fallback.
pub struct FallbackExtractor<P: Extractor, F: Extractor> {
    primary: P,
    fallback: F,
}

impl<P: Extractor, F: Extractor> FallbackExtractor<P, F> {
    /// Create a new fallback extractor.
    ///
    /// # Arguments
    /// * `primary` - The primary extractor to try first
    /// * `fallback` - The fallback extractor to try if primary fails
    pub fn new(primary: P, fallback: F) -> Self {
        Self { primary, fallback }
    }

    /// Get the primary extractor.
    pub fn primary(&self) -> &P {
        &self.primary
    }

    /// Get the fallback extractor.
    pub fn fallback(&self) -> &F {
        &self.fallback
    }
}

impl<P: Extractor + Clone, F: Extractor + Clone> Clone for FallbackExtractor<P, F> {
    fn clone(&self) -> Self {
        Self { primary: self.primary.clone(), fallback: self.fallback.clone() }
    }
}

impl<P: Extractor, F: Extractor> Extractor for FallbackExtractor<P, F> {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        // 1. Try primary extractor for all IDs
        let mut results = self.primary.extract_batch(video_ids);

        // 2. Collect failed IDs
        let failed_ids: Vec<String> = results
            .iter()
            .filter_map(|(id, result)| if result.is_err() { Some(id.clone()) } else { None })
            .collect();

        // 3. Retry failed IDs with fallback
        if !failed_ids.is_empty() {
            let sample_failures: Vec<String> = failed_ids
                .iter()
                .take(3)
                .filter_map(|id| {
                    results
                        .get(id)
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("track_id={id}: {err}"))
                })
                .collect();
            log::info!(
                "Primary extractor ({}) failed for {} IDs, trying fallback ({})... sample_errors={:?}",
                self.primary.name(),
                failed_ids.len(),
                self.fallback.name(),
                sample_failures
            );

            let fallback_results = self.fallback.extract_batch(&failed_ids);

            // 4. Merge fallback results (overwrite failures with successes)
            for (id, result) in fallback_results {
                if result.is_ok() {
                    log::debug!("Fallback succeeded for {}", id);
                }
                results.insert(id, result);
            }
        }

        results
    }

    fn name(&self) -> &'static str {
        // Return primary name since that's what we try first
        self.primary.name()
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        // Try primary first
        match self.primary.extract_one(video_id) {
            Ok(url) => Ok(url),
            Err(primary_err) => {
                log::info!(
                    "Primary extractor ({}) failed for track_id={}, trying fallback ({})...",
                    self.primary.name(),
                    video_id,
                    self.fallback.name()
                );

                // Try fallback
                self.fallback.extract_one(video_id).map_err(|fallback_err| {
                    // Both failed - return combined error
                    anyhow::anyhow!(
                        "Both extractors failed. {}: {}, {}: {}",
                        self.primary.name(),
                        primary_err,
                        self.fallback.name(),
                        fallback_err
                    )
                })
            }
        }
    }

    fn extract_one_fresh(&self, video_id: &str) -> Result<String> {
        // Fresh requests must be single-attempt at this layer.
        // Retry/escalation policy belongs to recovery boundaries.
        self.primary.extract_one_fresh(video_id)
    }

    fn clear_cache(&self) {
        self.primary.clear_cache();
        self.fallback.clear_cache();
    }

    fn is_cached(&self, video_id: &str) -> bool {
        self.primary.is_cached(video_id) || self.fallback.is_cached(video_id)
    }

    fn invalidate(&self, video_id: &str) {
        self.primary.invalidate(video_id);
        self.fallback.invalidate(video_id);
    }
}

// Send + Sync are automatically derived if P and F are Send + Sync
unsafe impl<P: Extractor, F: Extractor> Send for FallbackExtractor<P, F> {}
unsafe impl<P: Extractor, F: Extractor> Sync for FallbackExtractor<P, F> {}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use anyhow::anyhow;

    use super::*;

    struct SuccessExtractor;
    struct FailingExtractor;
    struct PartialExtractor;
    struct CountingSuccessExtractor {
        calls: Arc<AtomicUsize>,
    }

    impl Extractor for SuccessExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids.iter().map(|id| (id.clone(), Ok(format!("success_url_{}", id)))).collect()
        }

        fn name(&self) -> &'static str {
            "success"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    impl Extractor for FailingExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids.iter().map(|id| (id.clone(), Err(anyhow!("always fails")))).collect()
        }

        fn name(&self) -> &'static str {
            "failing"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    impl Extractor for PartialExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            video_ids
                .iter()
                .map(|id| {
                    if id.starts_with("fail_") {
                        (id.clone(), Err(anyhow!("partial failure")))
                    } else {
                        (id.clone(), Ok(format!("partial_url_{}", id)))
                    }
                })
                .collect()
        }

        fn name(&self) -> &'static str {
            "partial"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    impl Extractor for CountingSuccessExtractor {
        fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            video_ids
                .iter()
                .map(|id| (id.clone(), Ok(format!("counted_success_url_{}", id))))
                .collect()
        }

        fn name(&self) -> &'static str {
            "counting-success"
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    #[test]
    fn test_primary_succeeds() {
        let extractor = FallbackExtractor::new(SuccessExtractor, FailingExtractor);
        let url = extractor.extract_one("test").unwrap();
        assert_eq!(url, "success_url_test");
    }

    #[test]
    fn test_fallback_on_failure() {
        let extractor = FallbackExtractor::new(FailingExtractor, SuccessExtractor);
        let url = extractor.extract_one("test").unwrap();
        assert_eq!(url, "success_url_test");
    }

    #[test]
    fn test_both_fail() {
        let extractor = FallbackExtractor::new(FailingExtractor, FailingExtractor);
        let result = extractor.extract_one("test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Both extractors failed"));
    }

    #[test]
    fn test_batch_partial_fallback() {
        let extractor = FallbackExtractor::new(PartialExtractor, SuccessExtractor);

        let results = extractor.extract_batch(&["good".to_string(), "fail_bad".to_string()]);

        // "good" should come from primary
        assert_eq!(results.get("good").unwrap().as_ref().unwrap(), "partial_url_good");

        // "fail_bad" should come from fallback
        assert_eq!(results.get("fail_bad").unwrap().as_ref().unwrap(), "success_url_fail_bad");
    }

    #[test]
    fn test_extract_one_fresh_does_not_fallback() {
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let extractor = FallbackExtractor::new(
            FailingExtractor,
            CountingSuccessExtractor { calls: Arc::clone(&fallback_calls) },
        );

        let result = extractor.extract_one_fresh("test");

        assert!(result.is_err());
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }
}
