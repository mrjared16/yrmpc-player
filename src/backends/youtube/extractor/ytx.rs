//! YTX extractor - fast Go-based YouTube stream extractor.
//!
//! Uses `ytx music --bulk` for efficient batch extraction.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    thread,
};

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
        Self { cookies_path: Some(cookies_path) }
    }

    /// Parse a single NDJSON line from ytx output.
    fn parse_ndjson_line(line: &str) -> Option<(String, Result<String>)> {
        let json: serde_json::Value = serde_json::from_str(line).ok()?;
        let video_id = json
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| json.get("video_id").and_then(|v| v.as_str()))?
            .to_string();

        // Check for error response
        if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
            return Some((video_id, Err(anyhow!("ytx error: {}", error))));
        }

        // Extract video_id and url
        let url = json.get("url").and_then(|v| v.as_str())?.to_string();

        if url.is_empty() {
            return Some((video_id, Err(anyhow!("ytx returned empty URL"))));
        }

        Some((video_id, Ok(url)))
    }

    fn summarize_bulk_stderr(stderr: &str) -> Option<String> {
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            return None;
        }

        for line in trimmed.lines().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(error) = err_json.get("error").and_then(|e| e.as_str()) {
                    return Some(error.to_string());
                }
            }
            return Some(line.to_string());
        }

        None
    }

    fn build_bulk_failure_error(
        video_id: &str,
        status: Option<std::process::ExitStatus>,
        stderr_summary: Option<&str>,
    ) -> anyhow::Error {
        match (status.and_then(|s| s.code()), stderr_summary) {
            (Some(code), Some(stderr)) => {
                anyhow!("ytx bulk failed for {video_id} (exit {code}): {stderr}")
            }
            (Some(code), None) => anyhow!("ytx bulk failed for {video_id} (exit {code})"),
            (None, Some(stderr)) => anyhow!("ytx bulk failed for {video_id}: {stderr}"),
            (None, None) => anyhow!("ytx bulk failed for {video_id}: no response"),
        }
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

        let stderr_handle = child.stderr.take().map(|stderr| {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                let mut lines = Vec::new();
                for line in reader.lines().map_while(Result::ok) {
                    lines.push(line);
                }
                lines.join("\n")
            })
        });

        // Read NDJSON output line by line
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some((video_id, result)) = Self::parse_ndjson_line(&line) {
                    log::trace!("ytx bulk track_id={} result={}", video_id, result.is_ok());
                    results.insert(video_id, result);
                }
            }
        }

        // Wait for process to finish
        let status = child.wait().ok();
        let stderr = stderr_handle.and_then(|handle| handle.join().ok()).unwrap_or_default();
        let stderr_summary = Self::summarize_bulk_stderr(&stderr);

        // Mark any missing IDs as errors
        for id in video_ids {
            if !results.contains_key(id) {
                results.insert(
                    id.clone(),
                    Err(Self::build_bulk_failure_error(id, status, stderr_summary.as_deref())),
                );
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

        let result: serde_json::Value =
            serde_json::from_slice(&output.stdout).context("Failed to parse ytx output as JSON")?;

        let url = result
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ytx output missing url field"))?;

        if url.is_empty() {
            return Err(anyhow!("ytx returned empty URL"));
        }

        log::trace!("ytx extracted URL for track_id={} (len={})", video_id, url.len());
        Ok(url.to_string())
    }

    fn clear_cache(&self) {}

    fn is_cached(&self, _video_id: &str) -> bool {
        false
    }

    fn invalidate(&self, _video_id: &str) {}
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
    fn test_parse_ndjson_success_with_bulk_id_field() {
        let line = r#"{"id":"abc123","url":"https://example.com/stream","itag":141}"#;
        let (id, result) = YtxExtractor::parse_ndjson_line(line).unwrap();
        assert_eq!(id, "abc123");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/stream");
    }

    #[test]
    fn test_parse_ndjson_error_with_bulk_id_field() {
        let line = r#"{"id":"abc123","error":"video not playable: ERROR"}"#;
        let (id, result) = YtxExtractor::parse_ndjson_line(line).unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(result.unwrap_err().to_string(), "ytx error: video not playable: ERROR");
    }

    #[test]
    fn test_empty_batch() {
        let extractor = YtxExtractor::new();
        let results = extractor.extract_batch(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_summarize_bulk_stderr_prefers_json_error() {
        let stderr = "noise\n{\"error\":\"bulk auth failed\"}";
        assert_eq!(
            YtxExtractor::summarize_bulk_stderr(stderr).as_deref(),
            Some("bulk auth failed")
        );
    }

    #[test]
    fn test_build_bulk_failure_error_includes_status_and_summary() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .status()
            .expect("status should be available");

        let err = YtxExtractor::build_bulk_failure_error("abc123", Some(status), Some("quota"));
        assert_eq!(err.to_string(), "ytx bulk failed for abc123 (exit 7): quota");
    }
}
