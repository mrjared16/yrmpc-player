use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rmpc::backends::youtube::{YouTubeServer, config::ExtractorType};

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
}

fn find_default_cookie_file() -> Option<PathBuf> {
    // Check common locations for cookie file
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
    // Initialize logger so log::info!, log::debug! etc work
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Millis))
        .init();

    let args = Args::parse();

    // Determine cookie file path
    let cookie_path = args.cookies.or_else(find_default_cookie_file);

    log::info!("Starting YouTube daemon at {:?}", args.socket);
    if let Some(ref path) = cookie_path {
        log::info!("Using cookies from {:?}", path);
    }

    // Parse extractor type
    let extractor_type = match args.extractor.to_lowercase().as_str() {
        "ytx" => ExtractorType::Ytx,
        _ => ExtractorType::YtDlp, // Default to yt-dlp
    };
    log::info!("Using stream extractor: {:?}", extractor_type);

    // Create YouTube server
    let server = YouTubeServer::new(
        &args.socket,
        cookie_path.as_deref().and_then(|p| p.to_str()),
        extractor_type,
    )?;

    log::info!("YouTube daemon ready, entering event loop...");

    // Run server (blocks until shutdown)
    server.run()
}
