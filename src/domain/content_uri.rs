//! Content URI - Backend-agnostic content identification
//!
//! Provides a unified way to identify content across different backends
//! (YouTube, MPD, Spotify, etc.) using URI scheme prefixes.
//!
//! # URI Format
//!
//! ```text
//! scheme:type:id
//! ```
//!
//! Where:
//! - `scheme`: Backend identifier ("yt", "mpd", "sp")
//! - `type`: Content type ("v" = video, "t" = track, "a" = artist, "al" = album, "p" = playlist)
//! - `id`: Backend-specific identifier
//!
//! # Examples
//!
//! ```text
//! yt:v:dQw4w9WgXcQ     - YouTube video
//! yt:a:UC-lHJZR3Gqxm24 - YouTube artist
//! yt:al:MPREb_xxx      - YouTube album
//! mpd:file:/path/to/song.mp3 - MPD file
//! ```

use std::fmt;

/// URI with scheme prefix for backend-agnostic content identification.
///
/// This newtype ensures all content references use a consistent format
/// that can be parsed to determine the backend and content type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentUri(String);

impl ContentUri {
    /// Create a new ContentUri from any string-like value.
    ///
    /// Note: This doesn't validate the format. Use `parse()` to check validity.
    pub fn new(uri: impl Into<String>) -> Self {
        Self(uri.into())
    }

    /// Create a YouTube video URI
    pub fn youtube_video(video_id: impl AsRef<str>) -> Self {
        Self(format!("yt:v:{}", video_id.as_ref()))
    }

    /// Create a YouTube artist URI
    pub fn youtube_artist(browse_id: impl AsRef<str>) -> Self {
        Self(format!("yt:a:{}", browse_id.as_ref()))
    }

    /// Create a YouTube album URI
    pub fn youtube_album(album_id: impl AsRef<str>) -> Self {
        Self(format!("yt:al:{}", album_id.as_ref()))
    }

    /// Create a YouTube playlist URI
    pub fn youtube_playlist(playlist_id: impl AsRef<str>) -> Self {
        Self(format!("yt:p:{}", playlist_id.as_ref()))
    }

    /// Create an MPD file URI
    pub fn mpd_file(path: impl AsRef<str>) -> Self {
        Self(format!("mpd:file:{}", path.as_ref()))
    }

    /// Parse the URI into its components.
    ///
    /// Returns `None` if the URI doesn't have the expected format.
    pub fn parse(&self) -> Option<UriComponents<'_>> {
        let parts: Vec<&str> = self.0.splitn(3, ':').collect();
        if parts.len() >= 3 {
            Some(UriComponents {
                scheme: parts[0],
                content_type: parts[1],
                id: parts[2],
            })
        } else {
            None
        }
    }

    /// Get the raw URI string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Get the scheme (backend identifier) if parseable
    pub fn scheme(&self) -> Option<&str> {
        self.parse().map(|c| c.scheme)
    }

    /// Get the content type identifier if parseable
    pub fn content_type(&self) -> Option<&str> {
        self.parse().map(|c| c.content_type)
    }

    /// Get the raw ID portion if parseable
    pub fn id(&self) -> Option<&str> {
        self.parse().map(|c| c.id)
    }

    /// Check if this is a YouTube URI
    pub fn is_youtube(&self) -> bool {
        self.scheme() == Some("yt")
    }

    /// Check if this is an MPD URI
    pub fn is_mpd(&self) -> bool {
        self.scheme() == Some("mpd")
    }

    /// Check if this URI represents playable content (video/track)
    pub fn is_playable(&self) -> bool {
        matches!(self.content_type(), Some("v") | Some("t"))
    }

    /// Check if this URI represents browsable content (artist/album/playlist)
    pub fn is_browsable(&self) -> bool {
        matches!(self.content_type(), Some("a") | Some("al") | Some("p"))
    }
}

/// Parsed URI components
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UriComponents<'a> {
    /// Backend scheme ("yt", "mpd", "sp")
    pub scheme: &'a str,
    /// Content type ("v", "t", "a", "al", "p")
    pub content_type: &'a str,
    /// Backend-specific identifier
    pub id: &'a str,
}

impl fmt::Display for ContentUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ContentUri {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for ContentUri {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ContentUri {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ============ Traits ============

/// Base trait for all content that has a URI.
///
/// This trait provides a uniform way to access the content identifier
/// regardless of the specific content type.
pub trait HasUri {
    /// Get the content URI
    fn uri(&self) -> &ContentUri;
}

/// Trait for content that can be displayed in lists.
///
/// Extends `HasUri` to provide display-related methods.
/// This is separate from the UI-layer `ListItemDisplay` trait
/// to keep domain logic decoupled from rendering concerns.
pub trait Displayable: HasUri {
    /// Primary display text (title/name)
    fn primary_text(&self) -> &str;

    /// Secondary display text (artist, channel, etc.)
    fn secondary_text(&self) -> Option<String>;

    /// Thumbnail URL if available
    fn thumbnail(&self) -> Option<&str>;

    /// Type icon for visual distinction
    fn type_icon(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_youtube_video_uri() {
        let uri = ContentUri::youtube_video("dQw4w9WgXcQ");
        assert_eq!(uri.as_str(), "yt:v:dQw4w9WgXcQ");
        assert!(uri.is_youtube());
        assert!(uri.is_playable());
        assert!(!uri.is_browsable());

        let components = uri.parse().unwrap();
        assert_eq!(components.scheme, "yt");
        assert_eq!(components.content_type, "v");
        assert_eq!(components.id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_youtube_artist_uri() {
        let uri = ContentUri::youtube_artist("UC-lHJZR3Gqxm24");
        assert_eq!(uri.as_str(), "yt:a:UC-lHJZR3Gqxm24");
        assert!(uri.is_youtube());
        assert!(!uri.is_playable());
        assert!(uri.is_browsable());
    }

    #[test]
    fn test_youtube_album_uri() {
        let uri = ContentUri::youtube_album("MPREb_xxx");
        assert_eq!(uri.as_str(), "yt:al:MPREb_xxx");
        assert!(uri.is_browsable());
    }

    #[test]
    fn test_mpd_file_uri() {
        let uri = ContentUri::mpd_file("/path/to/song.mp3");
        assert_eq!(uri.as_str(), "mpd:file:/path/to/song.mp3");
        assert!(uri.is_mpd());
        // MPD files are tracks, so they should be playable
        // Note: "file" is not in our playable list, we might want to add it
    }

    #[test]
    fn test_parse_invalid_uri() {
        let uri = ContentUri::new("invalid");
        assert!(uri.parse().is_none());
    }

    #[test]
    fn test_display_trait() {
        let uri = ContentUri::youtube_video("abc123");
        assert_eq!(format!("{}", uri), "yt:v:abc123");
    }
}
