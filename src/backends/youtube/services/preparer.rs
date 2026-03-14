use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};

use super::preload_scheduler::TrackId;
use crate::backends::youtube::{
    audio::{MpvInput, cache::AudioCache, sources::concat::FfmpegConcatSource},
    protocol::play_intent::PreloadTier,
    url_resolver::UrlResolver,
};

/// Result of preparing a track for playback.
#[derive(Debug)]
pub struct PreparedPlayback {
    pub track_id: TrackId,
    pub input: MpvInput,
    pub mode: PlaybackMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMode {
    /// Using cached prefix + remote stream (best quality, gapless).
    Combined,
    /// Direct stream URL (fallback when prefix not ready in time).
    Direct,
}

#[derive(Debug, Clone)]
pub struct PlaybackPreparerConfig {
    /// Max wait time for Immediate tier before falling back to direct transport.
    pub direct_deadline_ms: u64,
}

impl Default for PlaybackPreparerConfig {
    fn default() -> Self {
        Self { direct_deadline_ms: 200 }
    }
}

pub struct PlaybackPreparer {
    url_resolver: Arc<UrlResolver>,
    cache: Arc<AudioCache>,
    config: PlaybackPreparerConfig,
}

impl PlaybackPreparer {
    pub fn new(
        url_resolver: Arc<UrlResolver>,
        cache: Arc<AudioCache>,
        config: PlaybackPreparerConfig,
    ) -> Self {
        Self { url_resolver, cache, config }
    }

    /// Prepare a track for playback according to its tier.
    ///
    /// - Immediate: wait up to `direct_deadline_ms`, then direct
    ///   fallback
    /// - Gapless/Eager/Background: wait for prefix completion
    pub async fn prepare(&self, track_id: &str, tier: PreloadTier) -> Result<PreparedPlayback> {
        let start = std::time::Instant::now();
        log::info!("[PREPARE] start track_id={} tier={:?}", track_id, tier);

        let stream_url =
            self.url_resolver.get_url(track_id).context("Failed to resolve stream URL")?;
        let url_time = start.elapsed();
        log::info!("[PREPARE] url_resolved track_id={} elapsed={:?}", track_id, url_time);

        let result = self.prepare_with_stream_url(track_id, tier, &stream_url).await;
        let total_time = start.elapsed();

        match &result {
            Ok(prepared) => log::info!(
                "[PREPARE] complete track_id={} mode={:?} elapsed={:?}",
                track_id,
                prepared.mode,
                total_time
            ),
            Err(e) => log::warn!(
                "[PREPARE] failed track_id={} elapsed={:?} error={}",
                track_id,
                total_time,
                e
            ),
        }

        result
    }

    pub(crate) async fn prepare_with_stream_url(
        &self,
        track_id: &str,
        tier: PreloadTier,
        stream_url: &str,
    ) -> Result<PreparedPlayback> {
        match tier {
            PreloadTier::Immediate => self.prepare_with_deadline(track_id, stream_url).await,
            PreloadTier::Gapless | PreloadTier::Eager | PreloadTier::Background => {
                self.prepare_with_prefix(track_id, stream_url).await
            }
        }
    }

    async fn prepare_with_deadline(
        &self,
        track_id: &str,
        stream_url: &str,
    ) -> Result<PreparedPlayback> {
        let deadline = Duration::from_millis(self.config.direct_deadline_ms);

        match race_prefix_with_deadline(deadline, self.cache.ensure_prefix(track_id, stream_url))
            .await
        {
            PrefixRace::Ready((prefix_path, content_length)) => {
                self.cache.touch(track_id);

                let input = self.build_concat_input(&prefix_path, content_length, stream_url);
                Ok(PreparedPlayback {
                    track_id: track_id.to_string(),
                    input,
                    mode: PlaybackMode::Combined,
                })
            }
            PrefixRace::Failed(e) => {
                log::warn!("Prefix download failed for {}, using direct transport: {}", track_id, e);
                self.build_direct(track_id, stream_url)
            }
            PrefixRace::TimedOut => {
                log::info!(
                    "Prefix not ready for {} within {}ms, using direct transport",
                    track_id,
                    self.config.direct_deadline_ms
                );
                self.build_direct(track_id, stream_url)
            }
        }
    }

    async fn prepare_with_prefix(
        &self,
        track_id: &str,
        stream_url: &str,
    ) -> Result<PreparedPlayback> {
        let (prefix_path, content_length) = self.cache.ensure_prefix(track_id, stream_url).await?;
        self.cache.touch(track_id);

        let input = self.build_concat_input(&prefix_path, content_length, stream_url);
        Ok(PreparedPlayback {
            track_id: track_id.to_string(),
            input,
            mode: PlaybackMode::Combined,
        })
    }

    fn build_concat_input(
        &self,
        prefix_path: &Path,
        content_length: u64,
        stream_url: &str,
    ) -> MpvInput {
        Self::build_concat_input_with_prefix_size(
            self.cache.prefix_size(),
            prefix_path,
            content_length,
            stream_url,
        )
    }

    fn build_concat_input_with_prefix_size(
        prefix_size: u64,
        prefix_path: &Path,
        content_length: u64,
        stream_url: &str,
    ) -> MpvInput {
        if prefix_size >= content_length {
            return MpvInput::new(prefix_path.to_string_lossy().to_string());
        }

        let concat_url = FfmpegConcatSource::build_concat_url(prefix_path, prefix_size, stream_url);
        MpvInput::with_args(concat_url, FfmpegConcatSource::protocol_whitelist_args())
    }

    fn build_direct(&self, track_id: &str, stream_url: &str) -> Result<PreparedPlayback> {
        Ok(PreparedPlayback {
            track_id: track_id.to_string(),
            input: MpvInput::new(stream_url.to_string()),
            mode: PlaybackMode::Direct,
        })
    }
}

#[derive(Debug)]
enum PrefixRace<T> {
    Ready(T),
    Failed(anyhow::Error),
    TimedOut,
}

async fn race_prefix_with_deadline<F, T>(deadline: Duration, future: F) -> PrefixRace<T>
where
    F: Future<Output = Result<T>>,
{
    match tokio::time::timeout(deadline, future).await {
        Ok(Ok(value)) => PrefixRace::Ready(value),
        Ok(Err(e)) => PrefixRace::Failed(e),
        Err(_) => PrefixRace::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::backends::youtube::audio::cache::CacheConfig;

    fn build_test_preparer(cache: Arc<AudioCache>) -> PlaybackPreparer {
        PlaybackPreparer::new(
            Arc::new(UrlResolver::default()),
            cache,
            PlaybackPreparerConfig::default(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn test_prepare_immediate_timeout_returns_direct() {
        let deadline = Duration::from_millis(200);

        let ensure_prefix = async {
            tokio::time::sleep(Duration::from_millis(5_000)).await;
            Ok::<_, anyhow::Error>((PathBuf::from("/tmp/prefix.webm"), 1_000u64))
        };

        let handle = tokio::spawn(race_prefix_with_deadline(deadline, ensure_prefix));
        tokio::time::advance(deadline).await;

        let outcome = handle.await.unwrap();
        assert!(matches!(outcome, PrefixRace::TimedOut));
    }

    #[tokio::test]
    async fn test_prepare_gapless_waits_for_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let prefix_size = 1024;
        let content_length = 10_000;

        let config = CacheConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            prefix_size,
            max_cache_size: 1024 * 1024,
        };

        let cache = Arc::new(AudioCache::new(config).unwrap());

        let track_id = "video1";
        let prefix_path = cache.cache_path(track_id);
        std::fs::write(&prefix_path, vec![0u8; prefix_size as usize]).unwrap();
        cache.register_prefix(track_id, prefix_path.clone(), prefix_size, content_length);

        let preparer = build_test_preparer(cache);
        let stream_url = "https://example.com/stream";

        let prepared = preparer
            .prepare_with_stream_url(track_id, PreloadTier::Gapless, stream_url)
            .await
            .unwrap();

        assert_eq!(prepared.mode, PlaybackMode::Combined);
        assert_eq!(prepared.track_id, track_id);

        let expected_url =
            FfmpegConcatSource::build_concat_url(&prefix_path, prefix_size, stream_url);
        assert_eq!(prepared.input.url, expected_url);
        assert_eq!(prepared.input.mpv_args, FfmpegConcatSource::protocol_whitelist_args());
    }

    #[tokio::test]
    async fn test_prepare_with_cached_prefix() {
        let temp_dir = TempDir::new().unwrap();

        let prefix_size = 2048;
        let content_length = 1024;

        let config = CacheConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            prefix_size,
            max_cache_size: 1024 * 1024,
        };

        let cache = Arc::new(AudioCache::new(config).unwrap());

        let track_id = "video1";
        let prefix_path = cache.cache_path(track_id);
        std::fs::write(&prefix_path, vec![0u8; content_length as usize]).unwrap();
        cache.register_prefix(track_id, prefix_path.clone(), content_length, content_length);

        let preparer = build_test_preparer(cache);
        let stream_url = "https://example.com/stream";

        let prepared = preparer
            .prepare_with_stream_url(track_id, PreloadTier::Eager, stream_url)
            .await
            .unwrap();

        assert_eq!(prepared.mode, PlaybackMode::Combined);
        assert_eq!(prepared.track_id, track_id);
        assert_eq!(prepared.input.url, prefix_path.to_string_lossy().to_string());
        assert!(prepared.input.mpv_args.is_empty());
    }
}
