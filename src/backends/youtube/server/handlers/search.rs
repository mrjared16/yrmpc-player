//! Search and browse handlers.

use std::sync::Arc;

use crate::backends::youtube::{
    services::ApiService,
    protocol::{BrowseEntry, ServerResponse, SongData, SearchItemData},
};

/// Handle Search command
pub fn handle_search(api: &Arc<ApiService>, query: &str) -> ServerResponse {
    match api.search_items(query) {
        Ok(items) => {
            ServerResponse::SearchResults(items.into_iter().map(SearchItemData::from).collect())
        }
        Err(e) => ServerResponse::Error(e.to_string()),
    }
}

/// Handle Browse command
pub fn handle_browse(api: &Arc<ApiService>, path: &str) -> ServerResponse {
    match api.browse(path) {
        Ok(songs) => {
            ServerResponse::BrowseResults(
                songs.into_iter().map(|s| BrowseEntry::File(SongData::from(s))).collect(),
            )
        }
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
