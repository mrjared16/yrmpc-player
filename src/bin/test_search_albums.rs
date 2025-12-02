use std::{fs, path::PathBuf};

use ytmapi_rs::{Client, YtMusicBuilder, auth::BrowserToken, query::SearchQuery};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let cookie_path = PathBuf::from(home).join(".config/rmpc/cookie.txt");

    println!("Loading cookies from: {:?}", cookie_path);
    let contents = fs::read_to_string(&cookie_path)?;

    let mut cookie_parts = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            let name = parts[5];
            let value = parts[6];
            cookie_parts.push(format!("{}={}", name, value));
        }
    }
    let cookie_string = cookie_parts.join("; ");

    let client = Client::new()?;
    let token = BrowserToken::from_str(&cookie_string, &client).await?;
    let api = YtMusicBuilder::new_with_client(client)
        .with_browser_token(token)
        .build()
        .expect("Failed to build API");

    let query_str = "Kim Long";
    println!("Searching for '{}'...", query_str);
    let query = SearchQuery::new(query_str);
    let results = api.query(query).await?;

    println!("Found results:");
    println!("Artists: {}", results.artists.len());
    println!("Albums: {}", results.albums.len());
    println!("Songs: {}", results.songs.len());
    println!("Videos: {}", results.videos.len());
    println!("Community Playlists: {}", results.community_playlists.len());
    println!("Featured Playlists: {}", results.featured_playlists.len());
    println!("Episodes: {}", results.episodes.len());
    println!("Profiles: {}", results.profiles.len());
    println!("Podcasts: {}", results.podcasts.len());

    if !results.albums.is_empty() {
        println!("\nFirst 3 Albums:");
        for album in results.albums.iter().take(3) {
            println!("- {} by {} (ID: {:?})", album.title, album.artist, album.album_id);
        }
    }

    Ok(())
}
