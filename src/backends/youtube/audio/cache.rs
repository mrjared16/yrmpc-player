use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;

use crate::{
    backends::youtube::media::AudioLoader,
    shared::{
        cache::{CacheConfig as SharedCacheConfig, DiskCache, DiskCacheValue, Weigher},
        dedup::Dedup,
    },
};

const DEFAULT_PREFIX_SIZE: u64 = 204_800; // 200KB
const DEFAULT_MAX_CACHE_SIZE: u64 = 209_715_200; // 200MB

/// Parse the `clen=` query parameter from a YouTube stream URL.
fn parse_clen_from_url(url: &str) -> Option<u64> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or(url);
    query
        .split('&')
        .find_map(|param| param.strip_prefix("clen="))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|len| *len > 0)
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub cache_dir: PathBuf,
    pub prefix_size: u64,
    pub max_cache_size: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        let cache_dir =
            dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("rmpc").join("audio");
        Self { cache_dir, prefix_size: DEFAULT_PREFIX_SIZE, max_cache_size: DEFAULT_MAX_CACHE_SIZE }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    content_length: u64,
}

impl DiskCacheValue for CacheEntry {
    fn disk_path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Copy)]
struct CacheEntryWeigher;

impl Weigher<String, CacheEntry> for CacheEntryWeigher {
    fn weight(&self, _key: &String, value: &CacheEntry) -> u64 {
        value.size
    }
}

#[derive(Debug, Clone)]
pub struct AudioCache {
    config: CacheConfig,
    entries: Arc<RwLock<DiskCache<String, CacheEntry, CacheEntryWeigher>>>,
    dedup: Dedup<String, Result<(PathBuf, u64), String>>,
}

impl AudioCache {
    pub fn new(config: CacheConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.cache_dir)
            .context("Failed to create audio cache directory")?;

        let estimated_entries =
            (config.max_cache_size / config.prefix_size.max(1)).saturating_add(1);
        let max_entries = usize::try_from(estimated_entries).unwrap_or(usize::MAX / 2);
        let cache_policy =
            SharedCacheConfig::new(max_entries.max(1)).with_max_weight(config.max_cache_size);
        let entries = DiskCache::with_weigher(cache_policy, CacheEntryWeigher);

        Ok(Self { config, entries: Arc::new(RwLock::new(entries)), dedup: Dedup::new() })
    }

    pub fn with_defaults() -> Result<Self> {
        Self::new(CacheConfig::default())
    }

    pub fn cache_path(&self, video_id: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{}.webm", video_id))
    }

    pub fn has_prefix(&self, video_id: &str) -> bool {
        self.get_content_length(video_id).is_some()
    }

    pub fn get_content_length(&self, video_id: &str) -> Option<u64> {
        let mut entries = self.entries.write().unwrap();
        entries.get(video_id).map(|entry| entry.content_length)
    }

    pub fn get_prefix_metadata(&self, video_id: &str) -> Option<(PathBuf, u64, u64)> {
        let mut entries = self.entries.write().unwrap();
        entries.get(video_id).map(|entry| (entry.path.clone(), entry.size, entry.content_length))
    }

    pub fn total_size(&self) -> u64 {
        let entries = self.entries.read().unwrap();
        entries.total_weight()
    }

    pub fn register_prefix(&self, video_id: &str, path: PathBuf, size: u64, content_length: u64) {
        let mut entries = self.entries.write().unwrap();
        entries.insert(video_id.to_string(), CacheEntry { path, size, content_length });
    }

    pub fn touch(&self, video_id: &str) {
        let mut entries = self.entries.write().unwrap();
        let _ = entries.touch(video_id);
    }

    pub fn evict_lru(&self) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        entries.evict_excess();
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
    pub async fn ensure_prefix(&self, video_id: &str, stream_url: &str) -> Result<(PathBuf, u64)> {
        let video_id_owned = video_id.to_string();
        let stream_url_owned = stream_url.to_string();

        self.dedup
            .get_or_init_with_timeout(video_id_owned.clone(), Duration::from_secs(30), || {
                let this = self.clone();
                async move {
                    this.ensure_prefix_inner(&video_id_owned, &stream_url_owned)
                        .await
                        .map_err(|e| e.to_string())
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .map_err(|e| anyhow::anyhow!(e))
    }

    async fn ensure_prefix_inner(
        &self,
        video_id: &str,
        stream_url: &str,
    ) -> Result<(PathBuf, u64)> {
        let start = std::time::Instant::now();

        if let Some(content_length) = self.get_content_length(video_id) {
            let path = self.cache_path(video_id);
            if path.exists() {
                self.touch(video_id);
                log::info!(
                    "[CACHE] hit video_id={} content_length={} current_url_clen={} path={} elapsed={:?}",
                    video_id,
                    content_length,
                    stream_url.split('&').find(|p| p.starts_with("clen=")).unwrap_or("?"),
                    path.display(),
                    start.elapsed()
                );
                return Ok((path, content_length));
            }
        }

        log::info!("[CACHE] miss video_id={} downloading prefix...", video_id);

        let path = self.cache_path(video_id);
        let prefix_size = self.config.prefix_size;
        let client = reqwest::Client::builder()
            .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)) // Force IPv4
            .build()
            .context("Failed to build HTTP client")?;

        // Download only the prefix using YouTube's &range= query parameter.
        //
        // CRITICAL: YouTube CDN tracks &range= (query param) and HTTP Range:
        // header as SEPARATE token budgets.  By using &range= here, the relay
        // upstream can later use HTTP Range: on the SAME cached URL without 403.
        let range_end = prefix_size.saturating_sub(1);
        let ranged_url = format!("{stream_url}&range=0-{range_end}");
        log::debug!("[CACHE] HTTP_START video_id={} &range=0-{}", video_id, range_end);
        let response =
            client.get(&ranged_url).send().await.context("Failed to request audio prefix")?;
        log::debug!("[CACHE] HTTP_DONE video_id={}", video_id);

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("Prefix request failed with status {}", status));
        }

        // Parse content_length from clen= in the stream URL (the &range=
        // response is HTTP 200 with no Content-Range header).
        let content_length =
            parse_clen_from_url(stream_url).context("Missing clen= in stream URL")?;

        let bytes = response.bytes().await.context("Failed to download audio prefix")?;
        let size = bytes.len() as u64;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &bytes).context("Failed to write audio prefix")?;

        self.register_prefix(video_id, path.clone(), size, content_length);
        self.evict_lru()?;

        log::info!(
            "[CACHE] downloaded prefix video_id={} prefix_size={} content_length={} elapsed={:?}",
            video_id,
            size,
            content_length,
            start.elapsed()
        );

        Ok((path, content_length))
    }
}

#[async_trait]
impl AudioLoader for AudioCache {
    async fn ensure_prefix(
        &self,
        track_id: &str,
        url: &str,
        _prefix_bytes: u64,
    ) -> Result<PathBuf> {
        // Delegate to existing method, ignore content_length in return
        // Note: We use config.prefix_size instead of prefix_bytes parameter
        // to maintain consistency with existing cache behavior
        let (path, _content_length) = self.ensure_prefix(track_id, url).await?;
        Ok(path)
    }

    fn is_cached(&self, track_id: &str) -> bool {
        self.cache_path(track_id).exists()
    }

    fn cached_path(&self, track_id: &str) -> Option<PathBuf> {
        let path = self.cache_path(track_id);
        path.exists().then_some(path)
    }
}
