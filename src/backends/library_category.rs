use serde::{Deserialize, Serialize};

/// Categories of library content available in YouTube Music
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LibraryCategory {
    /// User's saved playlists
    Playlists,
    /// User's saved albums
    Albums,
    /// User's followed artists
    Artists,
    /// User's liked songs
    Songs,
}

impl LibraryCategory {
    /// Get the path prefix for this category (used in LibraryPane navigation)
    pub fn path_prefix(&self) -> &'static str {
        match self {
            Self::Playlists => "library:playlists",
            Self::Albums => "library:albums",
            Self::Artists => "library:artists",
            Self::Songs => "library:songs",
        }
    }

    /// Parse category from path string
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "library:playlists" => Some(Self::Playlists),
            "library:albums" => Some(Self::Albums),
            "library:artists" => Some(Self::Artists),
            "library:songs" => Some(Self::Songs),
            _ => None,
        }
    }
}
