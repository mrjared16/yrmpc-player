use std::sync::Arc;

use anyhow::{Context, Result};

use crate::backends::youtube::audio::cache::AudioCache;
use crate::backends::youtube::audio::mpv_source::{MpvAudioSource, MpvInput};

type UrlResolver = Box<dyn Fn(&str) -> Result<String> + Send + Sync>;

pub struct ConcatSource {
    cache: Arc<AudioCache>,
    resolve_url: UrlResolver,
}

impl ConcatSource {
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

impl MpvAudioSource for ConcatSource {
    fn build_mpv_input(&mut self, video_id: &str) -> Result<MpvInput> {
        let stream_url = (self.resolve_url)(video_id)
            .context("Failed to resolve stream URL")?;

        let prefix_path = self.cache.cache_path(video_id);
        let prefix_size = self.cache.prefix_size();
        
        if let Some(content_length) = self.cache.get_content_length(video_id) {
            if prefix_path.exists() {
                self.cache.touch(video_id);
                
                if prefix_size >= content_length {
                    return Ok(MpvInput::new(prefix_path.to_string_lossy().to_string()));
                }
                
                let concat_url = Self::build_concat_url(&prefix_path, prefix_size, &stream_url);
                return Ok(MpvInput::with_args(concat_url, Self::protocol_whitelist_args()));
            }
        }

        Ok(MpvInput::new(stream_url))
    }
}
