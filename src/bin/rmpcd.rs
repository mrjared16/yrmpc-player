use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use rmpc::player::youtube::YouTubeServer;

#[derive(Parser, Debug)]
#[command(author, version, about = "YouTube Music daemon server", long_about = None)]
struct Args {
    /// Socket path for YouTube daemon
    #[arg(short, long, default_value = "/tmp/yrmpc-yt.sock")]
    socket: PathBuf,
    
    /// Cookie file for YouTube authentication (defaults to ~/.config/rmpc/cookie.txt)
    #[arg(short, long)]
    cookies: Option<PathBuf>,
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
            eprintln!("[INFO] Found cookie file at {:?}", loc);
            return Some(loc);
        }
    }
    
    eprintln!("[WARN] No cookie file found. Search, browse may not work.");
    eprintln!("[HINT] Export cookies from browser to ~/.config/rmpc/cookie.txt");
    None
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Determine cookie file path
    let cookie_path = args.cookies.or_else(find_default_cookie_file);
    
    eprintln!("[INFO] Starting YouTube daemon at {:?}", args.socket);
    if let Some(ref path) = cookie_path {
        eprintln!("[INFO] Using cookies from {:?}", path);
    }
    
    // Create YouTube server
    let server = YouTubeServer::new(
        &args.socket,
        cookie_path.as_deref().and_then(|p| p.to_str()),
    )?;
    
    eprintln!("[INFO] YouTube daemon ready, entering event loop...");
    
    // Run server (blocks until shutdown)
    server.run()
}
