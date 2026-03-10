use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rmpc::backends::youtube::{
    YouTubeServer,
    config::{AudioDeliveryMode, ExtractorType},
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

    /// Audio mode: combined (default), direct, or relay
    #[arg(short, long, default_value = "combined")]
    audio_source: String,
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

    let audio_delivery_mode = match args.audio_source.to_lowercase().as_str() {
        "direct" | "passthrough" => AudioDeliveryMode::Direct,
        "relay" | "proxy" => AudioDeliveryMode::Relay,
        "combined" | "concat" | "ffmpegconcat" => AudioDeliveryMode::Combined,
        other => {
            log::warn!("Unknown --audio-source='{other}', falling back to 'combined'");
            AudioDeliveryMode::Combined
        }
    };
    log::info!("Using audio mode: {:?}", audio_delivery_mode);

    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();

    let server = YouTubeServer::new(
        &args.socket,
        cookie_path.as_deref().and_then(|p| p.to_str()),
        extractor_type,
        audio_delivery_mode,
    )?;

    log::info!("YouTube daemon ready, entering event loop...");

    server.run()
}
