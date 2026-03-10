//! yt-dlp extractor - reliable fallback using yt-dlp CLI.
use std::{collections::HashMap, process::Command};

use anyhow::{Context, Result, anyhow};

use super::Extractor;

#[derive(Debug, Clone, Default)]
pub struct YtDlpExtractor;

impl YtDlpExtractor {
    pub fn new() -> Self {
        Self
    }

    fn parse_id_url_lines(stdout: &str) -> HashMap<String, String> {
        let mut by_id = HashMap::new();

        for line in stdout.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if let Some((id, url)) = line.split_once('\t') {
                let id = id.trim();
                let url = url.trim();

                if !id.is_empty() && !url.is_empty() {
                    by_id.insert(id.to_string(), url.to_string());
                }
            }
        }

        by_id
    }

    fn extract_bulk(video_ids: &[String]) -> HashMap<String, Result<String>> {
        if video_ids.is_empty() {
            return HashMap::new();
        }

        let urls: Vec<String> =
            video_ids.iter().map(|id| format!("https://music.youtube.com/watch?v={id}")).collect();

        let mut cmd = Command::new("yt-dlp");
        cmd.args(["-f", "774/141/251", "--ignore-errors", "--print", "%(id)s\t%(url)s"]);
        cmd.args(&urls);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(err) => {
                let msg = format!("Failed to run yt-dlp. Is it installed? {err}");
                return video_ids
                    .iter()
                    .cloned()
                    .map(|id| (id, Err(anyhow!(msg.clone()))))
                    .collect();
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let by_id = Self::parse_id_url_lines(&stdout);

        if !stderr.trim().is_empty() {
            log::debug!("yt-dlp stderr: {}", stderr.trim());
        }

        video_ids
            .iter()
            .cloned()
            .map(|id| {
                let result = if let Some(url) = by_id.get(&id) {
                    log::debug!("yt-dlp extracted URL for {} (len={})", id, url.len());
                    Ok(url.clone())
                } else {
                    let base = format!("No stream URL returned by yt-dlp for {id}");
                    if stderr.trim().is_empty() {
                        Err(anyhow!(base))
                    } else {
                        Err(anyhow!("{base}. stderr: {}", stderr.trim()))
                    }
                };

                (id, result)
            })
            .collect()
    }
}

impl Extractor for YtDlpExtractor {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        Self::extract_bulk(video_ids)
    }

    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        let id = video_id.to_string();
        let mut results = Self::extract_bulk(std::slice::from_ref(&id));
        results
            .remove(video_id)
            .unwrap_or_else(|| Err(anyhow!("No result returned for {video_id}")))
            .context("yt-dlp extraction failed")
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
    fn test_empty_batch() {
        let extractor = YtDlpExtractor::new();
        let results = extractor.extract_batch(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_id_url_lines() {
        let parsed =
            YtDlpExtractor::parse_id_url_lines("abc\thttps://u1\nignored\nxyz\thttps://u2\n");

        assert_eq!(parsed.get("abc").map(String::as_str), Some("https://u1"));
        assert_eq!(parsed.get("xyz").map(String::as_str), Some("https://u2"));
        assert_eq!(parsed.len(), 2);
    }
}
