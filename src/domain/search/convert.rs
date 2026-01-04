//! Conversions from ytmapi-rs types to SearchItem types
//!
//! These conversions are thin - minimal transformation, keep data close to
//! source.

use std::time::Duration;

use ytmapi_rs::common::YoutubeID;

use super::*;

// ============ Helper Functions ============

/// Parse duration string "MM:SS" or "HH:MM:SS" to Duration
pub fn parse_duration(s: &str) -> Option<Duration> {
    s.split(':')
        .try_fold(0u64, |acc, part| part.parse::<u64>().map(|v| acc * 60 + v).ok())
        .map(Duration::from_secs)
}

/// Get the best (largest) thumbnail from a list
pub fn best_thumbnail<'a, I>(thumbnails: I) -> Option<String>
where
    I: IntoIterator<Item = &'a ytmapi_rs::common::Thumbnail>,
{
    thumbnails.into_iter().max_by_key(|t| t.width).map(|t| t.url.clone())
}

// ============ From implementations for ytmapi-rs types ============

impl From<ytmapi_rs::parse::SearchResultSong> for SearchItem {
    fn from(r: ytmapi_rs::parse::SearchResultSong) -> Self {
        SearchItem::Playable(PlayableItem::Song(SongItem {
            video_id: r.video_id.get_raw().to_string(),
            title: r.title,
            artist: r.artist,
            album: r.album.map(|a| a.name),
            duration: parse_duration(&r.duration),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            explicit: matches!(r.explicit, ytmapi_rs::common::Explicit::IsExplicit),
        }))
    }
}

impl From<ytmapi_rs::parse::SearchResultArtist> for SearchItem {
    fn from(r: ytmapi_rs::parse::SearchResultArtist) -> Self {
        SearchItem::Browsable(BrowsableItem::Artist(ArtistItem {
            browse_id: Some(r.browse_id.get_raw().to_string()),
            name: r.artist,
            subscribers: r.subscribers,
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
        }))
    }
}

impl From<ytmapi_rs::parse::SearchResultAlbum> for SearchItem {
    fn from(r: ytmapi_rs::parse::SearchResultAlbum) -> Self {
        SearchItem::Browsable(BrowsableItem::Album(AlbumItem {
            album_id: r.album_id.get_raw().to_string(),
            title: r.title,
            artist: r.artist,
            year: Some(r.year),
            album_type: Some(format!("{:?}", r.album_type)),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            explicit: matches!(r.explicit, ytmapi_rs::common::Explicit::IsExplicit),
        }))
    }
}

impl From<ytmapi_rs::parse::SearchResultCommunityPlaylist> for SearchItem {
    fn from(r: ytmapi_rs::parse::SearchResultCommunityPlaylist) -> Self {
        SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
            playlist_id: r.playlist_id.get_raw().to_string(),
            title: r.title,
            author: r.author,
            track_count: Some(r.views), // views field often contains track count
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
        }))
    }
}

impl From<ytmapi_rs::parse::BasicSearchResultCommunityPlaylist> for SearchItem {
    fn from(r: ytmapi_rs::parse::BasicSearchResultCommunityPlaylist) -> Self {
        use ytmapi_rs::parse::BasicSearchResultCommunityPlaylist;
        match r {
            BasicSearchResultCommunityPlaylist::Playlist(p) => {
                SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                    playlist_id: p.playlist_id.get_raw().to_string(),
                    title: p.title,
                    author: p.author,
                    track_count: Some(p.views),
                    thumbnail: p.thumbnails.last().map(|t| t.url.clone()),
                }))
            }
            BasicSearchResultCommunityPlaylist::Podcast(podcast) => {
                // Convert podcast to playlist for now (simplified handling)
                SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                    playlist_id: podcast.podcast_id.get_raw().to_string(),
                    title: podcast.title,
                    author: podcast.publisher,
                    track_count: None,
                    thumbnail: podcast.thumbnails.last().map(|t| t.url.clone()),
                }))
            }
            _ => {
                // Handle future variants gracefully - create a placeholder playlist
                SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                    playlist_id: String::new(),
                    title: "Unknown Content".to_string(),
                    author: String::new(),
                    track_count: None,
                    thumbnail: None,
                }))
            }
        }
    }
}

impl From<ytmapi_rs::parse::SearchResultFeaturedPlaylist> for SearchItem {
    fn from(r: ytmapi_rs::parse::SearchResultFeaturedPlaylist) -> Self {
        SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
            playlist_id: r.playlist_id.get_raw().to_string(),
            title: r.title,
            author: r.author,
            track_count: Some(r.songs),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
        }))
    }
}

/// Convert video search result to SearchItem
impl TryFrom<ytmapi_rs::parse::SearchResultVideo> for SearchItem {
    type Error = ();

    fn try_from(r: ytmapi_rs::parse::SearchResultVideo) -> Result<Self, Self::Error> {
        match r {
            ytmapi_rs::parse::SearchResultVideo::Video {
                title,
                channel_name,
                video_id,
                views,
                length,
                thumbnails,
                ..
            } => Ok(SearchItem::Playable(PlayableItem::Video(VideoItem {
                video_id: video_id.get_raw().to_string(),
                title,
                channel: channel_name,
                views: Some(views),
                duration: parse_duration(&length),
                thumbnail: thumbnails.last().map(|t| t.url.clone()),
            }))),
            ytmapi_rs::parse::SearchResultVideo::VideoEpisode {
                title,
                channel_name,
                episode_id,
                thumbnails,
                ..
            } => Ok(SearchItem::Playable(PlayableItem::Video(VideoItem {
                video_id: episode_id.get_raw().to_string(),
                title,
                channel: channel_name,
                views: None,
                duration: None,
                thumbnail: thumbnails.last().map(|t| t.url.clone()),
            }))),
        }
    }
}

/// Convert TopResult to SearchItem - requires special handling
impl TryFrom<ytmapi_rs::parse::TopResult> for SearchItem {
    type Error = TopResultConversionError;

    fn try_from(r: ytmapi_rs::parse::TopResult) -> Result<Self, Self::Error> {
        use ytmapi_rs::parse::TopResultType;

        match r.result_type {
            Some(TopResultType::Song) | Some(TopResultType::Video) => {
                let video_id = r.video_id.ok_or(TopResultConversionError::MissingVideoId)?;
                Ok(SearchItem::Playable(PlayableItem::Song(SongItem {
                    video_id,
                    title: r.result_name,
                    artist: r.artist.unwrap_or_default(),
                    album: r.album,
                    duration: r.duration.and_then(|d| parse_duration(&d)),
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    explicit: false,
                })))
            }
            Some(TopResultType::Artist) => {
                // browse_id can be None for some artists - handle gracefully
                Ok(SearchItem::Browsable(BrowsableItem::Artist(ArtistItem {
                    browse_id: r.browse_id,
                    name: r.result_name,
                    subscribers: r.subscribers,
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                })))
            }
            Some(TopResultType::Album(_)) => {
                let album_id = r.browse_id.ok_or(TopResultConversionError::MissingBrowseId)?;
                Ok(SearchItem::Browsable(BrowsableItem::Album(AlbumItem {
                    album_id,
                    title: r.result_name,
                    artist: r.artist.unwrap_or_default(),
                    year: r.year,
                    album_type: None,
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    explicit: false,
                })))
            }
            Some(TopResultType::Playlist) => {
                let playlist_id = r.browse_id.ok_or(TopResultConversionError::MissingBrowseId)?;
                Ok(SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                    playlist_id,
                    title: r.result_name,
                    author: r.artist.unwrap_or_default(),
                    track_count: None,
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                })))
            }
            _ => {
                // Unknown type - try to make sense of it based on available fields
                if let Some(video_id) = r.video_id {
                    // Has video_id - treat as playable song/video
                    Ok(SearchItem::Playable(PlayableItem::Song(SongItem {
                        video_id,
                        title: r.result_name,
                        artist: r.artist.unwrap_or_default(),
                        album: r.album,
                        duration: r.duration.and_then(|d| parse_duration(&d)),
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        explicit: false,
                    })))
                } else if let Some(browse_id) = r.browse_id {
                    // Has browse_id - treat as browsable artist
                    Ok(SearchItem::Browsable(BrowsableItem::Artist(ArtistItem {
                        browse_id: Some(browse_id),
                        name: r.result_name,
                        subscribers: r.subscribers,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    })))
                } else if r.byline.is_some() || r.artist.is_some() {
                    // Has byline or artist but no IDs - this is a "featured" top result like "Jazz
                    // Radio" Treat as a playlist/station - it can be displayed
                    // but not played directly Use a synthetic ID so it can at
                    // least be shown
                    let author =
                        r.byline.or(r.artist).unwrap_or_else(|| "YouTube Music".to_string());
                    Ok(SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                        playlist_id: format!(
                            "featured:{}",
                            r.result_name.chars().take(20).collect::<String>()
                        ),
                        title: r.result_name,
                        author,
                        track_count: None,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    })))
                } else {
                    // No usable fields at all
                    Err(TopResultConversionError::UnknownType)
                }
            }
        }
    }
}

/// Errors that can occur when converting TopResult
#[derive(Debug, Clone, PartialEq)]
pub enum TopResultConversionError {
    MissingVideoId,
    MissingBrowseId,
    UnknownType,
}

impl std::fmt::Display for TopResultConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingVideoId => write!(f, "TopResult is missing video_id"),
            Self::MissingBrowseId => write!(f, "TopResult is missing browse_id"),
            Self::UnknownType => write!(f, "TopResult has unknown type and no ID"),
        }
    }
}

impl std::error::Error for TopResultConversionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("3:45"), Some(Duration::from_secs(225)));
        assert_eq!(parse_duration("1:00:00"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("invalid"), None);
    }
}
