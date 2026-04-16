use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use super::super::{
    audio::{AudioDeliveryPlan, cache::AudioCache},
    url_resolver::{ExpiredUrlRecoveryOutcome, ExpiredUrlRecoveryStep, UrlResolver},
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

    pub(super) async fn resolve_stream_url_fresh(&self, track_id: String) -> Result<String> {
        let resolver = Arc::clone(&self.url_resolver);
        let track_id_for_blocking = track_id;
        tokio::task::spawn_blocking(move || resolver.get_url_fresh(&track_id_for_blocking))
            .await
            .context("spawn_blocking failed")?
            .context("Failed to resolve fresh stream URL")
    }

    pub(super) async fn resolve_stream_urls(
        &self,
        track_ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, Result<String>>> {
        let resolver = Arc::clone(&self.url_resolver);
        tokio::task::spawn_blocking(move || resolver.get_urls(&track_ids))
            .await
            .context("spawn_blocking failed")
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
    ) -> Result<(PathBuf, u64, u64, String)> {
        let mut attempts = 0;
        let mut current_url = stream_url.to_string();
        let mut refreshed = false;
        let mut switched = false;

        loop {
            attempts += 1;
            match self.audio_cache.ensure_prefix(track_id, &current_url).await {
                Ok((prefix_path, content_length)) => {
                    let prefix_bytes = std::fs::metadata(&prefix_path)
                        .with_context(|| {
                            format!("Failed to read prefix metadata for {}", prefix_path.display())
                        })?
                        .len();
                    if refreshed || switched {
                        log::info!(
                            "[CACHE] 403 recovery track_id={} context=prefix attempts={} refreshed={} switched={} outcome=recovered",
                            track_id,
                            attempts,
                            refreshed,
                            switched,
                        );
                    }
                    return Ok((prefix_path, prefix_bytes, content_length, current_url.clone()));
                }
                Err(err) => {
                    let msg = err.to_string();
                    let is_403 = msg.contains("403") || msg.contains("Forbidden");
                    if !is_403 {
                        return Err(err);
                    }

                    if !refreshed {
                        refreshed = true;
                        match self.url_resolver.recover_after_expired(
                            track_id,
                            &current_url,
                            ExpiredUrlRecoveryStep::PrimaryRefresh,
                        )? {
                            ExpiredUrlRecoveryOutcome::Recovered(recovered_url) => {
                                current_url = recovered_url;
                                continue;
                            }
                            ExpiredUrlRecoveryOutcome::NoProgress => {}
                        }
                    }

                    if !switched {
                        switched = true;
                        match self.url_resolver.recover_after_expired(
                            track_id,
                            &current_url,
                            ExpiredUrlRecoveryStep::SwitchExtractor,
                        )? {
                            ExpiredUrlRecoveryOutcome::Recovered(recovered_url) => {
                                current_url = recovered_url;
                                continue;
                            }
                            ExpiredUrlRecoveryOutcome::NoProgress => {}
                        }
                    }

                    log::info!(
                        "[CACHE] 403 recovery track_id={} context=prefix attempts={} refreshed={} switched={} outcome=failed",
                        track_id,
                        attempts,
                        refreshed,
                        switched,
                    );
                    return Err(err);
                }
            }
        }
    }
    #[cfg(test)]
    pub(super) fn audio_cache(&self) -> Arc<AudioCache> {
        Arc::clone(&self.audio_cache)
    }
}
