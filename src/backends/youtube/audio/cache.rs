use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Instant;

use anyhow::{Context, Result};

const DEFAULT_PREFIX_SIZE: u64 = 204_800; // 200KB
const DEFAULT_MAX_CACHE_SIZE: u64 = 209_715_200; // 200MB

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub cache_dir: PathBuf,
    pub prefix_size: u64,
    pub max_cache_size: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rmpc")
            .join("audio");
        Self {
            cache_dir,
            prefix_size: DEFAULT_PREFIX_SIZE,
            max_cache_size: DEFAULT_MAX_CACHE_SIZE,
        }
    }
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    content_length: u64,
    last_accessed: Instant,
}

#[derive(Debug)]
pub struct AudioCache {
    config: CacheConfig,
    entries: RwLock<HashMap<String, CacheEntry>>,
}

impl AudioCache {
    pub fn new(config: CacheConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.cache_dir)
            .context("Failed to create audio cache directory")?;
        
        Ok(Self {
            config,
            entries: RwLock::new(HashMap::new()),
        })
    }

    pub fn with_defaults() -> Result<Self> {
        Self::new(CacheConfig::default())
    }

    pub fn cache_path(&self, video_id: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{}.m4a", video_id))
    }

    pub fn has_prefix(&self, video_id: &str) -> bool {
        let entries = self.entries.read().unwrap();
        entries.contains_key(video_id)
    }

    pub fn get_content_length(&self, video_id: &str) -> Option<u64> {
        let entries = self.entries.read().unwrap();
        entries.get(video_id).map(|e| e.content_length)
    }

    pub fn total_size(&self) -> u64 {
        let entries = self.entries.read().unwrap();
        entries.values().map(|e| e.size).sum()
    }

    pub fn register_prefix(
        &self,
        video_id: &str,
        path: PathBuf,
        size: u64,
        content_length: u64,
    ) {
        let mut entries = self.entries.write().unwrap();
        entries.insert(
            video_id.to_string(),
            CacheEntry {
                path,
                size,
                content_length,
                last_accessed: Instant::now(),
            },
        );
    }

    pub fn touch(&self, video_id: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(video_id) {
            entry.last_accessed = Instant::now();
        }
    }

    pub fn evict_lru(&self) -> Result<()> {
        while self.total_size() > self.config.max_cache_size {
            let oldest = {
                let entries = self.entries.read().unwrap();
                entries
                    .iter()
                    .min_by_key(|(_, e)| e.last_accessed)
                    .map(|(k, e)| (k.clone(), e.path.clone()))
            };

            if let Some((video_id, path)) = oldest {
                if path.exists() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("Failed to remove cache file: {:?}", path))?;
                }
                let mut entries = self.entries.write().unwrap();
                entries.remove(&video_id);
            } else {
                break;
            }
        }
        Ok(())
    }

    pub fn prefix_size(&self) -> u64 {
        self.config.prefix_size
    }

    /// Ensures the prefix is cached, downloading if necessary.
    ///
    /// Returns (path, content_length) where:
    /// - path: Path to the cached prefix file
    /// - content_length: Total file size (for byte offset calculation)
    pub async fn ensure_prefix(
        &self,
        video_id: &str,
        stream_url: &str,
    ) -> Result<(PathBuf, u64)> {
        if let Some(content_length) = self.get_content_length(video_id) {
            let path = self.cache_path(video_id);
            if path.exists() {
                self.touch(video_id);
                return Ok((path, content_length));
            }
        }

        let path = self.cache_path(video_id);
        let client = reqwest::Client::new();
        
        let range_header = format!("bytes=0-{}", self.config.prefix_size - 1);
        let response = client
            .get(stream_url)
            .header("Range", &range_header)
            .send()
            .await
            .context("Failed to request audio prefix")?;

        // Content-Range header format: "bytes 0-204799/12345678"
        let content_length = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split('/').last())
            .and_then(|s| s.parse::<u64>().ok())
            .context("Missing or invalid Content-Range header")?;

        let bytes = response.bytes().await.context("Failed to download prefix")?;
        let size = bytes.len() as u64;
        
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &bytes).context("Failed to write prefix file")?;

        self.register_prefix(video_id, path.clone(), size, content_length);
        self.evict_lru()?;

        Ok((path, content_length))
    }
}
