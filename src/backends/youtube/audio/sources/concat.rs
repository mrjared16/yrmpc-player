use anyhow::Result;

use crate::backends::youtube::{
    audio::mpv_source::MpvInput as MpvSourceInput,
    media::PreparedMedia,
};

pub struct FfmpegConcatSource;

impl FfmpegConcatSource {
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
    /// Without lavf://, MPV's [file] handler treats "concat:" as a filesystem
    /// path.
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

    pub fn build_from_prepared(prepared: &PreparedMedia) -> Result<MpvSourceInput> {
        match prepared {
            PreparedMedia::StagedPrefix { path, bytes, url, content_length } => {
                if bytes >= content_length {
                    Ok(MpvSourceInput::new(path.to_string_lossy().to_string()))
                } else {
                    let concat_url = Self::build_concat_url(path, *bytes, url);
                    Ok(MpvSourceInput::with_args(concat_url, Self::protocol_whitelist_args()))
                }
            }
            PreparedMedia::Direct { url } => Ok(MpvSourceInput::new(url.clone())),
            PreparedMedia::LocalFile { path } => {
                Ok(MpvSourceInput::new(path.to_string_lossy().to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::FfmpegConcatSource;
    use crate::backends::youtube::media::PreparedMedia;

    #[test]
    fn combined_adapter_builds_concat_input_from_prepared_staging() {
        let prepared = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 2048,
            url: "https://example.com/stream".to_string(),
            content_length: 4096,
        };

        let input = FfmpegConcatSource::build_from_prepared(&prepared).unwrap();

        assert_eq!(
            input.url,
            "lavf://concat:/tmp/prefix.webm|subfile,,start,2048,end,0,,:https://example.com/stream"
        );
        assert_eq!(input.mpv_args, FfmpegConcatSource::protocol_whitelist_args());
    }

    #[test]
    fn combined_adapter_uses_local_file_when_prepared_prefix_covers_track() {
        let prepared = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/full.webm"),
            bytes: 4096,
            url: "https://example.com/stream".to_string(),
            content_length: 4096,
        };

        let input = FfmpegConcatSource::build_from_prepared(&prepared).unwrap();

        assert_eq!(input.url, "/tmp/full.webm");
        assert!(input.mpv_args.is_empty());
    }

    #[test]
    fn combined_adapter_passes_direct_prepared_media_through() {
        let prepared = PreparedMedia::Direct { url: "https://example.com/direct".to_string() };

        let input = FfmpegConcatSource::build_from_prepared(&prepared).unwrap();

        assert_eq!(input.url, "https://example.com/direct");
        assert!(input.mpv_args.is_empty());
    }
}
