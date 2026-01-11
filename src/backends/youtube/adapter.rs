//! ytmapi-rs type conversions - ONLY file that imports ytmapi_rs types.
//!
//! This module isolates all ytmapi-rs dependencies. If upstream breaks,
//! only this file needs updating. Domain layer never sees ytmapi types.
//!
//! Flow: ytmapi_rs::* → api::Item → domain::SearchItem

use std::time::Duration;

use ytmapi_rs::{
    common::YoutubeID,
    parse::{
        BasicSearchResultCommunityPlaylist,
        SearchResultAlbum,
        SearchResultArtist,
        SearchResultCommunityPlaylist,
        SearchResultFeaturedPlaylist,
        SearchResultSong,
        SearchResultVideo,
        TopResult,
        TopResultType,
    },
};

use crate::backends::api::{ContentType, Item, SearchResults, SearchSection};

/// Parse duration string "MM:SS" or "HH:MM:SS" to Duration
fn parse_duration(s: &str) -> Option<Duration> {
    s.split(':')
        .try_fold(0u64, |acc, part| part.parse::<u64>().map(|v| acc * 60 + v).ok())
        .map(Duration::from_secs)
}

// ============================================================================
// Song conversions
// ============================================================================

impl From<SearchResultSong> for Item {
    fn from(r: SearchResultSong) -> Self {
        Item {
            id: r.video_id.get_raw().to_string(),
            content_type: ContentType::Track,
            title: r.title,
            subtitle: Some(r.artist),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            duration: parse_duration(&r.duration),
            queue_id: None,
        }
    }
}

// ============================================================================
// Artist conversions
// ============================================================================

impl From<SearchResultArtist> for Item {
    fn from(r: SearchResultArtist) -> Self {
        Item {
            id: r.browse_id.get_raw().to_string(),
            content_type: ContentType::Artist,
            title: r.artist,
            subtitle: r.subscribers,
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            duration: None,
            queue_id: None,
        }
    }
}

// ============================================================================
// Album conversions
// ============================================================================

impl From<SearchResultAlbum> for Item {
    fn from(r: SearchResultAlbum) -> Self {
        Item {
            id: r.album_id.get_raw().to_string(),
            content_type: ContentType::Album,
            title: r.title,
            subtitle: Some(r.artist),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            duration: None,
            queue_id: None,
        }
    }
}

// ============================================================================
// Playlist conversions
// ============================================================================

impl From<SearchResultCommunityPlaylist> for Item {
    fn from(r: SearchResultCommunityPlaylist) -> Self {
        Item {
            id: r.playlist_id.get_raw().to_string(),
            content_type: ContentType::Playlist,
            title: r.title,
            subtitle: Some(r.author),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            duration: None,
            queue_id: None,
        }
    }
}

impl From<BasicSearchResultCommunityPlaylist> for Item {
    fn from(r: BasicSearchResultCommunityPlaylist) -> Self {
        match r {
            BasicSearchResultCommunityPlaylist::Playlist(p) => Item {
                id: p.playlist_id.get_raw().to_string(),
                content_type: ContentType::Playlist,
                title: p.title,
                subtitle: Some(p.author),
                thumbnail: p.thumbnails.last().map(|t| t.url.clone()),
                duration: None,
                queue_id: None,
            },
            BasicSearchResultCommunityPlaylist::Podcast(podcast) => Item {
                id: podcast.podcast_id.get_raw().to_string(),
                content_type: ContentType::Playlist,
                title: podcast.title,
                subtitle: Some(podcast.publisher),
                thumbnail: podcast.thumbnails.last().map(|t| t.url.clone()),
                duration: None,
                queue_id: None,
            },
            _ => Item {
                id: String::new(),
                content_type: ContentType::Playlist,
                title: "Unknown Content".to_string(),
                subtitle: None,
                thumbnail: None,
                duration: None,
                queue_id: None,
            },
        }
    }
}

impl From<SearchResultFeaturedPlaylist> for Item {
    fn from(r: SearchResultFeaturedPlaylist) -> Self {
        Item {
            id: r.playlist_id.get_raw().to_string(),
            content_type: ContentType::Playlist,
            title: r.title,
            subtitle: Some(r.author),
            thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
            duration: None,
            queue_id: None,
        }
    }
}

// ============================================================================
// Video conversions
// ============================================================================

impl From<SearchResultVideo> for Item {
    fn from(r: SearchResultVideo) -> Self {
        match r {
            SearchResultVideo::Video {
                title, channel_name, video_id, length, thumbnails, ..
            } => Item {
                id: video_id.get_raw().to_string(),
                content_type: ContentType::Track,
                title,
                subtitle: Some(channel_name),
                thumbnail: thumbnails.last().map(|t| t.url.clone()),
                duration: parse_duration(&length),
                queue_id: None,
            },
            SearchResultVideo::VideoEpisode {
                title, channel_name, episode_id, thumbnails, ..
            } => Item {
                id: episode_id.get_raw().to_string(),
                content_type: ContentType::Track,
                title,
                subtitle: Some(channel_name),
                thumbnail: thumbnails.last().map(|t| t.url.clone()),
                duration: None,
                queue_id: None,
            },
        }
    }
}

// ============================================================================
// TopResult conversion - the complex one
// ============================================================================

/// Errors when converting TopResult
#[derive(Debug)]
pub enum TopResultError {
    MissingId { result_name: String, expected: &'static str },
    UnknownType { result_name: String, raw_type: Option<String> },
}

impl std::fmt::Display for TopResultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingId { result_name, expected } => {
                write!(f, "TopResult '{}' missing {}", result_name, expected)
            }
            Self::UnknownType { result_name, raw_type } => {
                write!(f, "TopResult '{}' has unknown type: {:?}", result_name, raw_type)
            }
        }
    }
}

impl TryFrom<TopResult> for Item {
    type Error = TopResultError;

    fn try_from(r: TopResult) -> Result<Self, Self::Error> {
        let result_name = r.result_name.clone();
        let raw_type = r.result_type.as_ref().map(|t| format!("{:?}", t));

        match r.result_type {
            Some(TopResultType::Song) | Some(TopResultType::Video) => {
                let id = r.video_id.ok_or(TopResultError::MissingId {
                    result_name: result_name.clone(),
                    expected: "video_id",
                })?;
                Ok(Item {
                    id,
                    content_type: ContentType::Track,
                    title: r.result_name,
                    subtitle: r.artist,
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    duration: r.duration.and_then(|d| parse_duration(&d)),
                    queue_id: None,
                })
            }
            Some(TopResultType::Artist) => Ok(Item {
                id: r.browse_id.unwrap_or_default(),
                content_type: ContentType::Artist,
                title: r.result_name,
                subtitle: r.subscribers,
                thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                duration: None,
                queue_id: None,
            }),
            Some(TopResultType::Album(_)) => {
                let id = r.browse_id.ok_or(TopResultError::MissingId {
                    result_name: result_name.clone(),
                    expected: "browse_id",
                })?;
                Ok(Item {
                    id,
                    content_type: ContentType::Album,
                    title: r.result_name,
                    subtitle: r.artist,
                    thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                    duration: None,
                    queue_id: None,
                })
            }
            Some(TopResultType::Playlist) => {
                if let Some(id) = r.video_id {
                    Ok(Item {
                        id,
                        content_type: ContentType::Track,
                        title: r.result_name,
                        subtitle: r.artist,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: r.duration.and_then(|d| parse_duration(&d)),
                        queue_id: None,
                    })
                } else {
                    let id = r.browse_id.ok_or(TopResultError::MissingId {
                        result_name: result_name.clone(),
                        expected: "browse_id",
                    })?;
                    Ok(Item {
                        id,
                        content_type: ContentType::Playlist,
                        title: r.result_name,
                        subtitle: r.artist,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: None,
                        queue_id: None,
                    })
                }
            }
            Some(TopResultType::Station) => {
                if let Some(id) = r.video_id {
                    Ok(Item {
                        id,
                        content_type: ContentType::Track,
                        title: r.result_name,
                        subtitle: r.artist,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: r.duration.and_then(|d| parse_duration(&d)),
                        queue_id: None,
                    })
                } else if let Some(id) = r.browse_id {
                    Ok(Item {
                        id,
                        content_type: ContentType::Playlist,
                        title: r.result_name,
                        subtitle: r.artist.or(Some("YouTube Music".to_string())),
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: None,
                        queue_id: None,
                    })
                } else {
                    Err(TopResultError::MissingId {
                        result_name,
                        expected: "video_id or browse_id",
                    })
                }
            }
            _ => {
                if let Some(id) = r.video_id {
                    Ok(Item {
                        id,
                        content_type: ContentType::Track,
                        title: r.result_name,
                        subtitle: r.artist,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: r.duration.and_then(|d| parse_duration(&d)),
                        queue_id: None,
                    })
                } else if let Some(id) = r.browse_id {
                    Ok(Item {
                        id,
                        content_type: ContentType::Artist,
                        title: r.result_name,
                        subtitle: r.subscribers,
                        thumbnail: r.thumbnails.last().map(|t| t.url.clone()),
                        duration: None,
                        queue_id: None,
                    })
                } else {
                    Err(TopResultError::UnknownType { result_name, raw_type })
                }
            }
        }
    }
}

// ============================================================================
// High-level conversion: ytmapi SearchResults → api::SearchResults
// ============================================================================

/// Convert ytmapi-rs search results to api::SearchResults
///
/// This is the main entry point - call this from YouTubeApi::search_items()
pub fn convert_search_results(results: ytmapi_rs::parse::SearchResults) -> SearchResults {
    let mut sections = Vec::new();

    // Top results
    if !results.top_results.is_empty() {
        let items: Vec<Item> =
            results.top_results.into_iter().filter_map(|r| Item::try_from(r).ok()).collect();
        if !items.is_empty() {
            sections.push(SearchSection {
                key: "top_results".to_string(),
                title: "Top Result".to_string(),
                items,
            });
        }
    }

    // Artists
    if !results.artists.is_empty() {
        let items: Vec<Item> = results.artists.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "artists".to_string(),
            title: "Artists".to_string(),
            items,
        });
    }

    // Albums
    if !results.albums.is_empty() {
        let items: Vec<Item> = results.albums.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "albums".to_string(),
            title: "Albums".to_string(),
            items,
        });
    }

    // Songs
    if !results.songs.is_empty() {
        let items: Vec<Item> = results.songs.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "songs".to_string(),
            title: "Songs".to_string(),
            items,
        });
    }

    // Videos
    if !results.videos.is_empty() {
        let items: Vec<Item> = results.videos.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "videos".to_string(),
            title: "Videos".to_string(),
            items,
        });
    }

    // Featured playlists
    if !results.featured_playlists.is_empty() {
        let items: Vec<Item> = results.featured_playlists.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "featured_playlists".to_string(),
            title: "Featured Playlists".to_string(),
            items,
        });
    }

    // Community playlists
    if !results.community_playlists.is_empty() {
        let items: Vec<Item> = results.community_playlists.into_iter().map(Item::from).collect();
        sections.push(SearchSection {
            key: "playlists".to_string(),
            title: "Playlists".to_string(),
            items,
        });
    }

    SearchResults { sections }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("3:45"), Some(Duration::from_secs(225)));
        assert_eq!(parse_duration("1:00:00"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("invalid"), None);
    }

    #[test]
    fn test_station_with_video_id_becomes_track() {
        let json = r#"{
            "result_name": "My Radio Station",
            "result_type": "Station",
            "thumbnails": [],
            "artist": "Test Artist",
            "album": null,
            "duration": "3:45",
            "year": null,
            "subscribers": null,
            "plays": null,
            "publisher": null,
            "byline": null,
            "browse_id": "RDAMVM_abc123",
            "video_id": "dQw4w9WgXcQ"
        }"#;

        let top_result: TopResult = serde_json::from_str(json).expect("parse");
        let item: Item = top_result.try_into().expect("convert");

        assert_eq!(item.id, "dQw4w9WgXcQ");
        assert_eq!(item.content_type, ContentType::Track);
    }

    #[test]
    fn test_playlist_with_video_id_becomes_track() {
        // YouTube returns songs with radio playlists as "Playlist" type
        // If video_id exists, user wants to PLAY the song, not browse
        let json = r#"{
            "result_name": "Kho Hon",
            "result_type": "Playlist",
            "thumbnails": [],
            "artist": "Mr Siro",
            "album": null,
            "duration": "4:32",
            "year": null,
            "subscribers": null,
            "plays": null,
            "publisher": null,
            "byline": null,
            "browse_id": "RDAMVM_abc123",
            "video_id": "dQw4w9WgXcQ"
        }"#;

        let top_result: TopResult = serde_json::from_str(json).expect("parse");
        let item: Item = top_result.try_into().expect("convert");

        // Should use video_id and be playable as Track
        assert_eq!(item.id, "dQw4w9WgXcQ");
        assert_eq!(item.content_type, ContentType::Track);
    }

    #[test]
    fn test_playlist_without_video_id_stays_playlist() {
        // Real playlist with no video_id - should remain browsable
        let json = r#"{
            "result_name": "My Playlist",
            "result_type": "Playlist",
            "thumbnails": [],
            "artist": "Creator",
            "album": null,
            "duration": null,
            "year": null,
            "subscribers": null,
            "plays": null,
            "publisher": null,
            "byline": null,
            "browse_id": "PLabc123",
            "video_id": null
        }"#;

        let top_result: TopResult = serde_json::from_str(json).expect("parse");
        let item: Item = top_result.try_into().expect("convert");

        assert_eq!(item.id, "PLabc123");
        assert_eq!(item.content_type, ContentType::Playlist);
    }
}
