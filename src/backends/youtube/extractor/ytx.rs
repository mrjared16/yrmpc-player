//! YTX extractor - fast Go-based YouTube stream extractor.
//!
//! Uses `ytx music --bulk` for efficient batch extraction.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use super::Extractor;

/// YTX extractor using the Go-based ytx binary.
///
/// Features:
/// - Bulk mode: `ytx music --bulk id1,id2,id3` (NDJSON output)
/// - 256kbps AAC audio quality
/// - ~500ms per extraction, with internal rate limiting
#[derive(Debug, Clone)]
pub struct YtxExtractor {
    /// Path to cookies file (optional, uses default if None)
    cookies_path: Option<String>,
}

impl YtxExtractor {
    /// Create a new YTX extractor with default cookies location.
    pub fn new() -> Self {
        Self { cookies_path: None }
    }

    /// Create a new YTX extractor with custom cookies path.
    pub fn with_cookies(cookies_path: String) -> Self {
        Self {
            cookies_path: Some(cookies_path),
        }
    }

    /// Parse a single NDJSON line from ytx output.
    fn parse_ndjson_line(line: &str) -> Option<(String, Result<String>)> {
        let json: serde_json::Value = serde_json::from_str(line).ok()?;

        // Check for error response
        if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
            let video_id = json
                .get("video_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Some((video_id, Err(anyhow!("ytx error: {}", error))));
        }

        // Extract video_id and url
        let video_id = json.get("video_id").and_then(|v| v.as_str())?.to_string();
        let url = json.get("url").and_then(|v| v.as_str())?.to_string();

        if url.is_empty() {
            return Some((video_id, Err(anyhow!("ytx returned empty URL"))));
        }

        Some((video_id, Ok(url)))
    }
}

impl Default for YtxExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for YtxExtractor {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        let mut results = HashMap::new();

        if video_ids.is_empty() {
            return results;
        }

        // Single ID: use simple mode (faster startup)
        if video_ids.len() == 1 {
            let id = &video_ids[0];
            let result = self.extract_one(id);
            results.insert(id.clone(), result);
            return results;
        }

        // Multiple IDs: use bulk mode
        let ids_csv = video_ids.join(",");
        let mut cmd = Command::new("ytx");
        cmd.args(["music", "--bulk", &ids_csv]);

        if let Some(ref cookies) = self.cookies_path {
            cmd.args(["--cookies", cookies]);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let err = anyhow!("Failed to spawn ytx: {}. Is it installed?", e);
                for id in video_ids {
                    results.insert(id.clone(), Err(anyhow!("{}", err)));
                }
                return results;
            }
        };

        // Read NDJSON output line by line
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some((video_id, result)) = Self::parse_ndjson_line(&line) {
                    log::debug!("ytx bulk: {} -> {}", video_id, result.is_ok());
                    results.insert(video_id, result);
                }
            }
        }

        // Wait for process to finish
        let _ = child.wait();

        // Mark any missing IDs as errors
        for id in video_ids {
            if !results.contains_key(id) {
                results.insert(id.clone(), Err(anyhow!("No response from ytx for {}", id)));
            }
        }

        results
    }

    fn name(&self) -> &'static str {
        "ytx"
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        let mut cmd = Command::new("ytx");
        cmd.args(["music", video_id]);

        if let Some(ref cookies) = self.cookies_path {
            cmd.args(["--cookies", cookies]);
        }

        let output = cmd
            .output()
            .context("Failed to run ytx. Is it installed? Build with: cd ytx && make build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(&stderr) {
                if let Some(error) = err_json.get("error").and_then(|e| e.as_str()) {
                    return Err(anyhow!("ytx failed: {}", error));
                }
            }
            return Err(anyhow!("ytx failed: {}", stderr.trim()));
        }

        let result: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("Failed to parse ytx output as JSON")?;

        let url = result
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ytx output missing url field"))?;

        if url.is_empty() {
            return Err(anyhow!("ytx returned empty URL"));
        }

        log::debug!("ytx extracted URL for {} (len={})", video_id, url.len());
        Ok(url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ndjson_success() {
        let line = r#"{"video_id":"abc123","url":"https://example.com/stream","itag":141}"#;
        let (id, result) = YtxExtractor::parse_ndjson_line(line).unwrap();
        assert_eq!(id, "abc123");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/stream");
    }

    #[test]
    fn test_parse_ndjson_error() {
        let line = r#"{"video_id":"abc123","error":"Video not found"}"#;
        let (id, result) = YtxExtractor::parse_ndjson_line(line).unwrap();
        assert_eq!(id, "abc123");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_batch() {
        let extractor = YtxExtractor::new();
        let results = extractor.extract_batch(&[]);
        assert!(results.is_empty());
    }
}
