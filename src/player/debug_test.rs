
#[cfg(test)]
mod debug_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use crate::config::YouTubeConfig;
    use crate::player::youtube_backend::YouTubeBackend;
    use crate::player::backend::MusicBackend;
    use crate::app_state::AppState;
    use crate::mpd::mpd_client::{Filter, Tag};

    #[test]
    fn debug_search_backend() {
        let home = std::env::var("HOME").unwrap();
        let cookie_path = PathBuf::from(home).join(".config/rmpc/cookie.txt");
        
        let config = YouTubeConfig {
            auth_file: Some(cookie_path.to_string_lossy().to_string()),
        };

        let app_state = Arc::new(RwLock::new(AppState::new()));
        let socket_path = PathBuf::from("/tmp/rmpc-debug-mpv.sock");

        println!("Initializing YouTubeBackend...");
        let mut backend = YouTubeBackend::new(app_state, &socket_path, config).unwrap();

        let queries = vec!["The Beatles", "Kim Long"];

        for q in queries {
            println!("\n=== SEARCH: '{}' ===", q);
            let filter = Filter::new(Tag::Any, q);
            let results = backend.search(&[filter]).unwrap();
            
            println!("Found {} items.", results.len());
            for (i, song) in results.iter().enumerate() {
                let type_ = song.metadata.get("type").and_then(|v| v.first()).map(|s| s.as_str()).unwrap_or("UNKNOWN");
                println!("[{}] Type: {}, ID: '{}', Title: '{}'", i, type_, song.file, song.metadata.get("title").and_then(|v| v.first()).unwrap_or(&"No Title".to_string()));
                
                if type_ == "song" || type_ == "video" {
                    if song.file.contains(":") || song.file.len() < 10 {
                         println!("    WARNING: Suspicious Video ID!");
                    }
                }
            }
        }

        println!("\n=== LIBRARY: Playlists ===");
        let playlists = backend.list_playlists().unwrap();
        println!("Found {} playlists.", playlists.len());
        for p in playlists {
            println!("- {} ({})", p.name, p.full_path);
        }
    }
}
