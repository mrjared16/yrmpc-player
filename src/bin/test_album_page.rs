use std::{fs, path::PathBuf};

use ytmapi_rs::{
    Client,
    YtMusicBuilder,
    auth::BrowserToken,
    common::{AlbumID, YoutubeID},
    query::GetAlbumQuery,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let cookie_path = PathBuf::from(home).join(".config/rmpc/cookie.txt");

    let contents = fs::read_to_string(&cookie_path)?;
    let mut cookie_parts = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 7 {
            cookie_parts.push(format!("{}={}", parts[5], parts[6]));
        }
    }
    let cookie_string = cookie_parts.join("; ");

    let client = Client::new()?;
    let token = BrowserToken::from_str(&cookie_string, &client).await?;
    let api = YtMusicBuilder::new_with_client(client)
        .with_browser_token(token)
        .build()
        .expect("Failed to build API");

    let album_id = "MPREb_GYegtrrVvE8";
    println!("Fetching album page for {}...", album_id);

    let query = GetAlbumQuery::new(AlbumID::from_raw(album_id));
    let album = api.query(query).await?;

    println!("Album Title: {}", album.title);
    println!("Tracks found: {}", album.tracks.len());

    for track in album.tracks {
        println!("- {} ({:?})", track.title, track.video_id);
    }

    Ok(())
}
