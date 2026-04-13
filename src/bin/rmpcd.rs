#![allow(
    clippy::doc_markdown,
    clippy::uninlined_format_args,
    clippy::unnecessary_debug_formatting,
    clippy::collapsible_if
)]

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rmpc::backends::youtube::{
    YouTubeServer,
    config::{AudioDeliveryMode, ExtractorType, YouTubeConfig, YtDlpExtractorConfig},
};

#[derive(Parser, Debug)]
#[command(author, version, about = "YouTube Music daemon server", long_about = None)]
struct Args {
    /// Socket path for YouTube daemon
    #[arg(short, long, default_value = "/tmp/yrmpc-yt.sock")]
    socket: PathBuf,

    /// Cookie file for YouTube authentication (defaults to
    /// ~/.config/rmpc/cookie.txt)
    #[arg(short, long)]
    cookies: Option<PathBuf>,

    /// Stream URL extractor: ytdlp (default, reliable) or ytx (faster)
    #[arg(short, long, default_value = "ytdlp")]
    extractor: String,

    /// Audio delivery mode: auto (default), direct, relay, or staged/concat
    #[arg(short, long = "audio-delivery", visible_alias = "audio-source", default_value = "auto")]
    audio_delivery: String,
}

fn parse_audio_delivery_mode(raw: &str) -> AudioDeliveryMode {
    match raw.to_lowercase().as_str() {
        "auto" => AudioDeliveryMode::Auto,
        "direct" | "passthrough" => AudioDeliveryMode::Direct,
        "relay" | "proxy" => AudioDeliveryMode::Relay,
        "staged" | "combined" | "concat" | "ffmpegconcat" => {
            log::warn!(
                "Audio delivery mode '{raw}' uses staged/concat playback, which has known 403/URL expiry issues; prefer '--audio-delivery relay'"
            );
            AudioDeliveryMode::Staged
        }
        other => {
            log::warn!("Unknown --audio-delivery='{other}', falling back to 'auto'");
            AudioDeliveryMode::Auto
        }
    }
}

fn find_default_cookie_file() -> Option<PathBuf> {
    let locations = vec![
        dirs::config_dir().map(|d| d.join("rmpc/cookie.txt")),
        dirs::config_dir().map(|d| d.join("yrmpc/cookies.txt")),
        dirs::home_dir().map(|d| d.join(".config/rmpc/cookie.txt")),
    ];

    for loc in locations.into_iter().flatten() {
        if loc.exists() {
            log::info!("Found cookie file at {:?}", loc);
            return Some(loc);
        }
    }

    log::warn!("No cookie file found. Search, browse may not work.");
    log::info!("HINT: Export cookies from browser to ~/.config/rmpc/cookie.txt");
    None
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Millis))
        .init();

    let args = Args::parse();
    let youtube_config = match YouTubeConfig::load() {
        Ok(config) => config,
        Err(err) => {
            log::warn!("Failed to load ~/.config/yrmpc/youtube.toml: {err}");
            YouTubeConfig::default()
        }
    };

    if std::env::var_os("YTMAPI_DEBUG_DIR").is_none() {
        if let Some(dump_dir) = youtube_config.api.request_dump_dir.clone() {
            ytmapi_rs::debug::set_debug_dir(dump_dir.clone());
            log::info!("Enabled ytmapi request dumps in {:?}", dump_dir);
        }
    }

    let cookie_path = args.cookies.or_else(find_default_cookie_file);

    log::info!("Starting YouTube daemon at {:?}", args.socket);
    if let Some(ref path) = cookie_path {
        log::info!("Using cookies from {:?}", path);
    }

    let extractor_type = match args.extractor.to_lowercase().as_str() {
        "ytx" => ExtractorType::Ytx,
        _ => ExtractorType::YtDlp,
    };
    log::info!("Using stream extractor: {:?}", extractor_type);

    let audio_delivery_mode = parse_audio_delivery_mode(&args.audio_delivery);
    log::info!("Using audio mode: {:?}", audio_delivery_mode);

    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();

    let server = YouTubeServer::new(
        &args.socket,
        cookie_path.as_deref().and_then(|p| p.to_str()),
        extractor_type,
        audio_delivery_mode,
        youtube_config.audio.background_extract_mode,
        youtube_config.audio.future_track_count,
        YtDlpExtractorConfig {
            cookies_path: cookie_path.as_deref().and_then(|path| path.to_str()).map(str::to_owned),
        },
    )?;

    log::info!("YouTube daemon ready, entering event loop...");

    server.run()
}

#[cfg(test)]
mod tests {
    use super::{Args, parse_audio_delivery_mode};
    use clap::Parser;
    use rmpc::backends::youtube::config::AudioDeliveryMode;

    #[test]
    fn audio_delivery_flag_defaults_to_auto() {
        let args = Args::parse_from(["rmpcd"]);
        assert_eq!(args.audio_delivery, "auto");
    }

    #[test]
    fn parse_audio_delivery_mode_keeps_legacy_aliases() {
        assert_eq!(parse_audio_delivery_mode("auto"), AudioDeliveryMode::Auto);
        assert_eq!(parse_audio_delivery_mode("combined"), AudioDeliveryMode::Staged);
        assert_eq!(parse_audio_delivery_mode("ffmpegconcat"), AudioDeliveryMode::Staged);
        assert_eq!(parse_audio_delivery_mode("concat"), AudioDeliveryMode::Staged);
        assert_eq!(parse_audio_delivery_mode("passthrough"), AudioDeliveryMode::Direct);
        assert_eq!(parse_audio_delivery_mode("proxy"), AudioDeliveryMode::Relay);
    }
}
