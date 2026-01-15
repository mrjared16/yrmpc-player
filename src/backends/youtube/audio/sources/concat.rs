use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::runtime::Handle;

use crate::backends::youtube::audio::{
    cache::AudioCache,
    mpv_source::{MpvAudioSource, MpvInput},
};

type UrlResolver = Box<dyn Fn(&str) -> Result<String> + Send + Sync>;

pub struct FfmpegConcatSource {
    cache: Arc<AudioCache>,
    resolve_url: UrlResolver,
    runtime_handle: Handle,
}

impl FfmpegConcatSource {
    pub fn new(cache: Arc<AudioCache>, resolve_url: UrlResolver) -> Self {
        let runtime_handle = Handle::try_current()
            .unwrap_or_else(|_| {
                log::warn!("No tokio runtime at FfmpegConcatSource construction, creating new one");
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.handle().clone()
            });
        Self { cache, resolve_url, runtime_handle }
    }

    pub fn with_runtime(cache: Arc<AudioCache>, resolve_url: UrlResolver, handle: Handle) -> Self {
        Self { cache, resolve_url, runtime_handle: handle }
    }

    /// MPV args needed to enable FFmpeg concat protocol.
    /// Uses --stream-lavf-o-append (not --demuxer-lavf-o) because we need the
    /// protocol whitelist at the STREAM layer, not just the demuxer layer.
    /// The concat: URL is wrapped with lavf:// to force FFmpeg stream handler.
    pub(crate) fn protocol_whitelist_args() -> Vec<String> {
        vec![
            "--stream-lavf-o-append=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat"
                .to_string(),
        ]
    }

    /// Build concat URL with lavf:// wrapper to force FFmpeg stream handler.
    /// Without lavf://, MPV's [file] handler treats "concat:" as a filesystem path.
    pub(crate) fn build_concat_url(
        prefix_path: &std::path::Path,
        prefix_size: u64,
        stream_url: &str,
    ) -> String {
        // Use lavf:// wrapper to force FFmpeg's libavformat stream handler
        format!(
            "lavf://concat:{}|subfile,,start,{},end,0,,:{}",
            prefix_path.display(),
            prefix_size,
            stream_url
        )
    }
}

impl MpvAudioSource for FfmpegConcatSource {
    fn build_mpv_input(&mut self, video_id: &str) -> Result<MpvInput> {
        let stream_url = (self.resolve_url)(video_id).context("Failed to resolve stream URL")?;

        let (prefix_path, content_length) =
            self.runtime_handle.block_on(self.cache.ensure_prefix(video_id, &stream_url))?;

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
