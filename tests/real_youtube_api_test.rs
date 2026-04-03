//! Real YouTube API integration tests
//! These tests actually call the YouTube Music API to verify the full stack
//! works.
//!
//! IMPORTANT: These tests are IGNORED by default to avoid network calls in CI.
//!
//! Run explicitly with:
//!   cargo test --test real_youtube_api_test -- --ignored --nocapture
//!
//! Prerequisites:
//! - Valid cookies.txt in project root (Netscape format)
//! - Internet connection

use std::path::PathBuf;

/// Find cookies.txt by walking up from the test directory
fn find_cookies_file() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("../cookies.txt"),
        PathBuf::from("../../cookies.txt"),
        PathBuf::from("cookies.txt"),
    ];

    for path in candidates {
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn parse_cookies(path: &PathBuf) -> String {
    let contents = std::fs::read_to_string(path).expect("Failed to read cookies");
    let mut parts = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let p: Vec<&str> = line.split('\t').collect();
        if p.len() >= 7 {
            parts.push(format!("{}={}", p[5], p[6]));
        }
    }
    parts.join("; ")
}

/// Test: Dump RAW ytmapi-rs response to see what YouTube actually returns
#[test]
#[ignore = "requires network and valid cookies.txt"]
fn test_raw_api_response() {
    let cookie_path = match find_cookies_file() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cookies.txt not found");
            return;
        }
    };

    let cookie_string = parse_cookies(&cookie_path);
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    let result = rt.block_on(async {
        use ytmapi_rs::{Client, YtMusicBuilder, auth::BrowserToken, query::SearchQuery};

        let client = Client::new()?;
        let token = BrowserToken::from_str(&cookie_string, &client).await?;
        let api = YtMusicBuilder::new_with_client(client).with_browser_token(token).build()?;

        println!("Searching for 'kho hon'...");
        let results = api.query(SearchQuery::new("kho hon")).await?;

        Ok::<_, anyhow::Error>(results)
    });

    match result {
        Ok(results) => {
            println!("\n========== RAW YTMAPI-RS RESPONSE ==========\n");

            println!(">>> TOP RESULTS ({} items):", results.top_results.len());
            for (i, r) in results.top_results.iter().enumerate() {
                println!("[{}] FULL STRUCT: {:#?}", i, r);
            }

            println!("\n>>> SONGS ({} items):", results.songs.len());
            for (i, r) in results.songs.iter().take(2).enumerate() {
                println!("[{}] {:#?}", i, r);
            }

            println!("\n>>> VIDEOS ({} items):", results.videos.len());
            for (i, r) in results.videos.iter().take(2).enumerate() {
                println!("[{}] {:#?}", i, r);
            }

            println!("\n>>> ALBUMS ({} items):", results.albums.len());
            for (i, r) in results.albums.iter().take(2).enumerate() {
                println!("[{}] {:#?}", i, r);
            }

            println!("\n>>> ARTISTS ({} items):", results.artists.len());
            for (i, r) in results.artists.iter().take(2).enumerate() {
                println!("[{}] {:#?}", i, r);
            }

            println!("\n>>> COMMUNITY PLAYLISTS ({} items):", results.community_playlists.len());
            for (i, r) in results.community_playlists.iter().take(2).enumerate() {
                println!("[{}] {:#?}", i, r);
            }

            println!("\n>>> FEATURED PLAYLISTS ({} items):", results.featured_playlists.len());
        }
        Err(e) => panic!("Search failed: {}", e),
    }
}

/// Test: rmpc wrapper search_items works
#[test]
#[ignore = "requires network and valid cookies.txt"]
fn test_rmpc_youtube_api_wrapper() {
    use rmpc::backends::youtube::api::YouTubeApi;

    let cookie_path = match find_cookies_file() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: cookies.txt not found");
            return;
        }
    };

    let api = YouTubeApi::new().expect("Failed to create YouTubeApi");
    api.load_cookies(cookie_path.to_str().unwrap()).expect("Failed to load cookies");

    assert!(api.is_authenticated(), "API should be authenticated");

    // Search for "kho hon" to test the Playlist+video_id case
    let search_result = api.search_items("kho hon");
    match search_result {
        Ok(results) => {
            println!("rmpc wrapper search returned {} sections", results.sections.len());
            for section in &results.sections {
                println!("  Section '{}': {} items", section.title, section.items.len());
                for (i, item) in section.items.iter().enumerate() {
                    println!("    [{}] {:?}", i, item);
                }
            }

            // Check if top_results has playable item
            if let Some(top) = results.sections.iter().find(|s| s.key == "top_results") {
                println!("\nTop Result section found with {} items", top.items.len());
            } else {
                println!("\nWARNING: No Top Result section - conversion may be broken");
            }
        }
        Err(e) => panic!("Search failed: {}", e),
    }
}
