use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use super::super::{
    audio::{AudioDeliveryPlan, cache::AudioCache},
    url_resolver::UrlResolver,
};

#[derive(Debug)]
pub(super) struct StagingPipeline {
    url_resolver: Arc<UrlResolver>,
    audio_cache: Arc<AudioCache>,
    uses_local_staging: bool,
}

impl StagingPipeline {
    pub(super) fn new(
        url_resolver: Arc<UrlResolver>,
        audio_cache: Arc<AudioCache>,
        audio_source_plan: AudioDeliveryPlan,
    ) -> Self {
        Self {
            url_resolver,
            audio_cache,
            uses_local_staging: audio_source_plan.uses_local_staging(),
        }
    }

    pub(super) fn uses_local_staging(&self) -> bool {
        self.uses_local_staging
    }

    pub(super) async fn resolve_stream_url(&self, track_id: String) -> Result<String> {
        let resolver = Arc::clone(&self.url_resolver);
        let track_id_for_blocking = track_id;
        tokio::task::spawn_blocking(move || resolver.get_url(&track_id_for_blocking))
            .await
            .context("spawn_blocking failed")?
            .context("Failed to resolve stream URL")
    }

    pub(super) fn prefetch_stream_urls(&self, track_ids: Vec<String>) {
        self.url_resolver.prefetch(track_ids);
    }

    pub(super) fn get_prefix_metadata(&self, track_id: &str) -> Option<(PathBuf, u64, u64)> {
        self.audio_cache.get_prefix_metadata(track_id)
    }

    pub(super) fn prefix_path_for(&self, track_id: &str) -> PathBuf {
        self.audio_cache.cache_path(track_id)
    }

    pub(super) fn default_prefix_size(&self) -> u64 {
        self.audio_cache.prefix_size()
    }

    pub(super) async fn ensure_prefix(
        &self,
        track_id: &str,
        stream_url: &str,
    ) -> Result<(PathBuf, u64, u64)> {
        self.audio_cache.ensure_prefix(track_id, stream_url).await.and_then(
            |(prefix_path, content_length)| {
                let prefix_bytes = std::fs::metadata(&prefix_path)
                    .with_context(|| {
                        format!("Failed to read prefix metadata for {}", prefix_path.display())
                    })?
                    .len();
                Ok((prefix_path, prefix_bytes, content_length))
            },
        )
    }
    #[cfg(test)]
    pub(super) fn audio_cache(&self) -> Arc<AudioCache> {
        Arc::clone(&self.audio_cache)
    }
}
