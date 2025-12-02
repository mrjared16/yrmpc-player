use std::{fs, path::PathBuf};

use ytmapi_rs::{
    Client,
    YtMusicBuilder,
    auth::BrowserToken,
    common::{ArtistChannelID, YoutubeID},
    query::GetArtistQuery,
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

    let artist_id = "UCVgjiGiG5DkpgKWKNj8tXiQ"; // Kim Long
    println!("Fetching artist page for {}...", artist_id);

    let query = GetArtistQuery::new(ArtistChannelID::from_raw(artist_id));
    let artist = api.query(query).await?;

    println!("Artist Name: {}", artist.name);
    // println!("Description: {:?}", artist.description);

    if let Some(songs) = artist.top_releases.songs {
        println!("Top Songs: {}", songs.results.len());
        for song in songs.results {
            println!("- {} (Album: {})", song.title, song.album.name);
        }
    } else {
        println!("No Top Songs found");
    }

    if let Some(albums) = artist.top_releases.albums {
        println!("Albums: {}", albums.results.len());
    }

    Ok(())
}
