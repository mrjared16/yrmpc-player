use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::runtime::Handle;

use crate::backends::youtube::audio::cache::AudioCache;
use crate::backends::youtube::audio::mpv_source::{MpvAudioSource, MpvInput};

type UrlResolver = Box<dyn Fn(&str) -> Result<String> + Send + Sync>;

pub struct FfmpegConcatSource {
    cache: Arc<AudioCache>,
    resolve_url: UrlResolver,
}

impl FfmpegConcatSource {
    pub fn new(cache: Arc<AudioCache>, resolve_url: UrlResolver) -> Self {
        Self { cache, resolve_url }
    }

    pub(crate) fn protocol_whitelist_args() -> Vec<String> {
        vec![
            "--demuxer-lavf-o=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat".to_string(),
        ]
    }

    pub(crate) fn build_concat_url(
        prefix_path: &std::path::Path,
        prefix_size: u64,
        stream_url: &str,
    ) -> String {
        format!(
            "concat:{}|subfile,,start,{},end,0,,:{}",
            prefix_path.display(),
            prefix_size,
            stream_url
        )
    }
}

impl MpvAudioSource for FfmpegConcatSource {
    fn build_mpv_input(&mut self, video_id: &str) -> Result<MpvInput> {
        let stream_url = (self.resolve_url)(video_id)
            .context("Failed to resolve stream URL")?;

        let handle = Handle::try_current()
            .context("No tokio runtime available for cache download")?;
        
        let (prefix_path, content_length) = handle.block_on(
            self.cache.ensure_prefix(video_id, &stream_url)
        )?;

        let prefix_size = self.cache.prefix_size();
        self.cache.touch(video_id);

        if prefix_size >= content_length {
            return Ok(MpvInput::new(prefix_path.to_string_lossy().to_string()));
        }

        let concat_url = Self::build_concat_url(&prefix_path, prefix_size, &stream_url);
        Ok(MpvInput::with_args(concat_url, Self::protocol_whitelist_args()))
    }

    fn has_cached(&self, video_id: &str) -> bool {
        self.cache.has_prefix(video_id)
    }
}
