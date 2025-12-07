//! Displayable trait for UI rendering
//!
//! UI widgets depend on this trait, not concrete types (SOLID: Dependency Inversion)

use super::*;

/// Abstraction for displaying search items in UI
pub trait Displayable {
    /// Primary text (title/name)
    fn primary_line(&self) -> &str;
    
    /// Secondary text (artist · album · duration)
    fn secondary_line(&self) -> Option<String>;
    
    /// Thumbnail URL if available
    fn thumbnail(&self) -> Option<&str>;
    
    /// Icon for this item type
    fn type_icon(&self) -> &'static str;
}

// ============ SearchItem ============

impl Displayable for SearchItem {
    fn primary_line(&self) -> &str {
        match self {
            Self::Playable(p) => p.primary_line(),
            Self::Browsable(b) => b.primary_line(),
            Self::Header(h) => h,
        }
    }

    fn secondary_line(&self) -> Option<String> {
        match self {
            Self::Playable(p) => p.secondary_line(),
            Self::Browsable(b) => b.secondary_line(),
            Self::Header(_) => None,
        }
    }

    fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Playable(p) => p.thumbnail(),
            Self::Browsable(b) => b.thumbnail(),
            Self::Header(_) => None,
        }
    }

    fn type_icon(&self) -> &'static str {
        match self {
            Self::Playable(p) => p.type_icon(),
            Self::Browsable(b) => b.type_icon(),
            Self::Header(_) => "─",
        }
    }
}

// ============ PlayableItem ============

impl Displayable for PlayableItem {
    fn primary_line(&self) -> &str {
        match self {
            Self::Song(s) => &s.title,
            Self::Video(v) => &v.title,
        }
    }

    fn secondary_line(&self) -> Option<String> {
        match self {
            Self::Song(s) => {
                let mut parts = vec![s.artist.clone()];
                if let Some(ref album) = s.album {
                    parts.push(album.clone());
                }
                if let Some(dur) = s.duration {
                    parts.push(format_duration(dur));
                }
                Some(parts.join(" · "))
            }
            Self::Video(v) => {
                let mut parts = vec![v.channel.clone()];
                if let Some(ref views) = v.views {
                    parts.push(views.clone());
                }
                if let Some(dur) = v.duration {
                    parts.push(format_duration(dur));
                }
                Some(parts.join(" · "))
            }
        }
    }

    fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Song(s) => s.thumbnail.as_deref(),
            Self::Video(v) => v.thumbnail.as_deref(),
        }
    }

    fn type_icon(&self) -> &'static str {
        match self {
            Self::Song(_) => "♪",
            Self::Video(_) => "▶",
        }
    }
}

// ============ BrowsableItem ============

impl Displayable for BrowsableItem {
    fn primary_line(&self) -> &str {
        match self {
            Self::Artist(a) => &a.name,
            Self::Album(a) => &a.title,
            Self::Playlist(p) => &p.title,
        }
    }

    fn secondary_line(&self) -> Option<String> {
        match self {
            Self::Artist(a) => a.subscribers.clone(),
            Self::Album(a) => {
                let mut parts = vec![a.artist.clone()];
                if let Some(ref year) = a.year {
                    parts.push(year.clone());
                }
                Some(parts.join(" · "))
            }
            Self::Playlist(p) => {
                let mut parts = vec![p.author.clone()];
                if let Some(ref count) = p.track_count {
                    parts.push(format!("{} tracks", count));
                }
                Some(parts.join(" · "))
            }
        }
    }

    fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Artist(a) => a.thumbnail.as_deref(),
            Self::Album(a) => a.thumbnail.as_deref(),
            Self::Playlist(p) => p.thumbnail.as_deref(),
        }
    }

    fn type_icon(&self) -> &'static str {
        match self {
            Self::Artist(_) => "👤",
            Self::Album(_) => "💿",
            Self::Playlist(_) => "📁",
        }
    }
}

// ============ Helpers ============

fn format_duration(dur: std::time::Duration) -> String {
    let secs = dur.as_secs();
    let mins = secs / 60;
    let secs = secs % 60;
    if mins >= 60 {
        let hours = mins / 60;
        let mins = mins % 60;
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(std::time::Duration::from_secs(65)), "1:05");
        assert_eq!(format_duration(std::time::Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn test_song_displayable() {
        let song = SongItem {
            video_id: "abc".into(),
            title: "Test Song".into(),
            artist: "Test Artist".into(),
            album: Some("Test Album".into()),
            duration: Some(std::time::Duration::from_secs(180)),
            thumbnail: None,
            explicit: false,
        };
        let playable = PlayableItem::Song(song);
        
        assert_eq!(playable.primary_line(), "Test Song");
        assert_eq!(playable.secondary_line(), Some("Test Artist · Test Album · 3:00".into()));
        assert_eq!(playable.type_icon(), "♪");
    }
}
