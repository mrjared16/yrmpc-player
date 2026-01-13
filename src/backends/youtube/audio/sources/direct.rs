use anyhow::{Context, Result};

use crate::backends::youtube::audio::mpv_source::{MpvAudioSource, MpvInput};

type UrlResolver = Box<dyn Fn(&str) -> Result<String> + Send + Sync>;

/// Direct URL streaming to MPV.
///
/// This is the simplest audio source strategy - it resolves the stream URL
/// and passes it directly to MPV with no caching or protocol manipulation.
///
/// **Behavior**:
/// - Resolves video_id → stream URL via yt-dlp/ytx
/// - Passes URL directly to MPV
/// - No local caching, no prefix download
/// - Startup latency depends on YouTube's initial response
///
/// **Use case**: Default/fallback when caching is disabled or unavailable.
pub struct DirectSource {
    resolve_url: UrlResolver,
}

impl DirectSource {
    pub fn new(resolve_url: UrlResolver) -> Self {
        Self { resolve_url }
    }
}

impl MpvAudioSource for DirectSource {
    fn build_mpv_input(&mut self, video_id: &str) -> Result<MpvInput> {
        let stream_url = (self.resolve_url)(video_id)
            .context("Failed to resolve stream URL")?;

        Ok(MpvInput::new(stream_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_source_returns_url() {
        let resolver = Box::new(|id: &str| -> Result<String> {
            Ok(format!("https://example.com/stream/{}", id))
        });

        let mut source = DirectSource::new(resolver);
        let input = source.build_mpv_input("abc123").unwrap();

        assert_eq!(input.url, "https://example.com/stream/abc123");
        assert!(input.mpv_args.is_empty());
    }

    #[test]
    fn test_direct_source_propagates_resolver_error() {
        let resolver = Box::new(|_: &str| -> Result<String> {
            anyhow::bail!("URL resolution failed")
        });

        let mut source = DirectSource::new(resolver);
        let result = source.build_mpv_input("abc123");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("resolve stream URL"));
    }
}
