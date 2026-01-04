//! yt-dlp extractor - reliable fallback using yt-dlp CLI.
//!
//! Uses sequential extraction to avoid rate limiting.

use std::{collections::HashMap, process::Command};

use anyhow::{Context, Result, anyhow};

use super::Extractor;

/// yt-dlp extractor using the yt-dlp CLI.
///
/// Features:
/// - Reliable, widely used
/// - ~3-4s per extraction
/// - Sequential processing to avoid rate limits
#[derive(Debug, Clone, Default)]
pub struct YtDlpExtractor;

impl YtDlpExtractor {
    /// Create a new yt-dlp extractor.
    pub fn new() -> Self {
        Self
    }

    /// Extract URL for a single video using yt-dlp.
    fn extract_single(video_id: &str) -> Result<String> {
        let video_url = format!("https://www.youtube.com/watch?v={}", video_id);

        let output = Command::new("yt-dlp")
            .args(["-f", "bestaudio", "-g", &video_url])
            .output()
            .context("Failed to run yt-dlp. Is it installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("yt-dlp failed: {}", stderr.trim()));
        }

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            return Err(anyhow!("yt-dlp returned empty URL"));
        }

        log::debug!("yt-dlp extracted URL for {} (len={})", video_id, url.len());
        Ok(url)
    }
}

impl Extractor for YtDlpExtractor {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        let mut results = HashMap::new();

        // Sequential extraction to avoid rate limiting
        // yt-dlp doesn't have native bulk mode, and parallel spawning
        // risks getting banned by YouTube
        for video_id in video_ids {
            log::debug!(
                "yt-dlp extracting {} ({}/{})",
                video_id,
                results.len() + 1,
                video_ids.len()
            );
            let result = Self::extract_single(video_id);
            results.insert(video_id.clone(), result);
        }

        results
    }

    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        Self::extract_single(video_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_batch() {
        let extractor = YtDlpExtractor::new();
        let results = extractor.extract_batch(&[]);
        assert!(results.is_empty());
    }
}
