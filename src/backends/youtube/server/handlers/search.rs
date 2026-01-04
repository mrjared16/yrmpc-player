//! Search and browse handlers.

use std::sync::Arc;

use crate::{
    backends::youtube::{
        protocol::{
            AlbumDetailsData,
            ArtistDetailsData,
            BrowseEntry,
            PlaylistDetailsData,
            ServerResponse,
            SongData,
        },
        services::ApiService,
    },
    domain::MediaItem,
};

/// Handle Search command - returns MediaItem directly (no intermediate types)
pub fn handle_search(api: &Arc<ApiService>, query: &str) -> ServerResponse {
    match api.search_items(query) {
        Ok(results) => {
            // Convert domain search results to MediaItem directly
            // Preserve section structure by inserting Header markers before each section's
            // items
            let mut items: Vec<MediaItem> = Vec::new();

            for section in results.sections {
                // Insert section header marker
                items.push(MediaItem::header(&section.title));

                // Add all items in this section (preserving their types for correct actions)
                for item in section.items {
                    use crate::domain::search::Displayable;
                    log::trace!(
                        "[DIAG-IMG] handle_search: section='{}' item='{}' thumbnail={:?}",
                        section.title,
                        item.primary_line(),
                        item.thumbnail()
                    );
                    items.push(MediaItem::from(item));
                }
            }

            log::info!(
                "[SEARCH] Returning {} MediaItem entries with section headers for query '{}'",
                items.len(),
                query
            );

            ServerResponse::SearchResults(items)
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle Browse command
pub fn handle_browse(api: &Arc<ApiService>, path: &str) -> ServerResponse {
    match api.browse(path) {
        Ok(songs) => ServerResponse::BrowseResults(
            songs.into_iter().map(|s| BrowseEntry::File(SongData::from(s))).collect(),
        ),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle GetSearchSuggestions command
pub fn handle_get_suggestions(api: &Arc<ApiService>, query: &str) -> ServerResponse {
    match api.get_suggestions(query) {
        Ok(suggestions) => ServerResponse::Suggestions(suggestions),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle GetLibrary command
pub fn handle_get_library(_category: &str) -> ServerResponse {
    // TODO: Implement library browsing
    ServerResponse::Library(vec![])
}

/// Handle BrowsePlaylistDetails command - returns rich playlist info
pub fn handle_browse_playlist_details(api: &Arc<ApiService>, playlist_id: &str) -> ServerResponse {
    match api.get_playlist_details(playlist_id) {
        Ok(details) => ServerResponse::PlaylistDetails(PlaylistDetailsData::from(details)),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle BrowseAlbumDetails command - returns rich album info
pub fn handle_browse_album_details(api: &Arc<ApiService>, album_id: &str) -> ServerResponse {
    match api.get_album_details(album_id) {
        Ok(details) => ServerResponse::AlbumDetails(AlbumDetailsData::from(details)),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle BrowseArtistDetails command - returns rich artist info
pub fn handle_browse_artist_details(api: &Arc<ApiService>, artist_id: &str) -> ServerResponse {
    match api.get_artist_details(artist_id) {
        Ok(details) => ServerResponse::ArtistDetails(ArtistDetailsData::from(details)),
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}
