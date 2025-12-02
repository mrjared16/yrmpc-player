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
    println!("Top Results: {}", results.top_results.len());
    println!("Artists: {}", results.artists.len());
    println!("Albums: {}", results.albums.len());
    println!("Songs: {}", results.songs.len());

    if !results.top_results.is_empty() {
        println!("\nFirst Top Result:");
        let top = &results.top_results[0];
        println!(
            "- {} (Type: {:?}, BrowseID: {:?}, VideoID: {:?})",
            top.result_name, top.result_type, top.browse_id, top.video_id
        );
    }

    if !results.artists.is_empty() {
        println!("\nFirst 3 Artists:");
        for artist in results.artists.iter().take(3) {
            println!("- {} (ID: {:?})", artist.artist, artist.browse_id);
        }
    }

    if !results.songs.is_empty() {
        println!("\nFirst 3 Songs:");
        for song in results.songs.iter().take(3) {
            println!("- {} - {} (ID: {:?})", song.title, song.artist, song.video_id);
        }
    }

    Ok(())
}
