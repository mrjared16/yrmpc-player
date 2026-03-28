//! Command handlers for YouTube server.
//!
//! Each handler module contains thin wrappers that:
//! 1. Validate input
//! 2. Call orchestrator or services
//! 3. Return ServerResponse
//!
//! Handlers are organized by domain:
//! - `playback`: Play, Pause, Stop, Seek, Next, Previous
//! - `queue`: Add, Delete, Clear, Move
//! - `status`: GetStatus, GetCurrentSong, GetPlaylist
//! - `search`: Search, Browse, GetSearchSuggestions
//! - `options`: SetRepeat, SetShuffle, Volume operations

pub mod options;
pub mod play_intent;
pub mod playback;
pub mod queue;
pub mod queue_events;
pub mod search;
pub mod status;

pub(super) fn extract_video_id(uri: &str) -> Option<String> {
    if let Some(id) = uri.strip_prefix("youtube://") {
        return Some(id.to_string());
    }

    if !uri.is_empty() && !uri.contains("://") {
        return Some(uri.to_string());
    }

    let parsed = url::Url::parse(uri).ok()?;
    match parsed.host_str()? {
        "youtu.be" => {
            parsed.path_segments()?.find(|segment| !segment.is_empty()).map(ToString::to_string)
        }
        "www.youtube.com" | "youtube.com" | "music.youtube.com" => {
            if parsed.path() == "/watch" {
                parsed
                    .query_pairs()
                    .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))
            } else if let Some(id) = parsed.path().strip_prefix("/shorts/") {
                (!id.is_empty()).then(|| id.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn stable_track_id(uri: &str) -> String {
    extract_video_id(uri).unwrap_or_else(|| uri.to_string())
}

pub use options::*;
pub use play_intent::*;
pub use playback::*;
pub use queue::*;
pub use search::*;
pub use status::*;

#[cfg(test)]
mod tests {
    use super::extract_video_id;

    #[test]
    fn extract_video_id_accepts_supported_shapes() {
        assert_eq!(extract_video_id("video123").as_deref(), Some("video123"));
        assert_eq!(extract_video_id("youtube://video123").as_deref(), Some("video123"));
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=video123").as_deref(),
            Some("video123")
        );
        assert_eq!(
            extract_video_id("https://music.youtube.com/watch?v=video123&si=abc").as_deref(),
            Some("video123")
        );
        assert_eq!(extract_video_id("https://youtu.be/video123").as_deref(), Some("video123"));
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/video123").as_deref(),
            Some("video123")
        );
    }

    #[test]
    fn extract_video_id_rejects_unrelated_urls() {
        assert_eq!(extract_video_id("https://example.com/watch?v=video123"), None);
        assert_eq!(extract_video_id(""), None);
    }
}
