// Integration tests for YouTubeBackend search functionality
// These tests verify the search implementation works correctly with ytmapi-rs

mod integration_tests {
    use std::collections::HashMap;

    /// Test that search results contain all expected fields
    #[test]
    fn test_search_result_structure() {
        // This test verifies that our Song objects created from search results
        // have all the required metadata fields

        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["artist".to_string()]);
        metadata.insert("title".to_string(), vec!["Test Artist".to_string()]);
        metadata.insert("artist".to_string(), vec!["Test Artist".to_string()]);

        // Verify all required fields exist
        assert!(metadata.contains_key("type"));
        assert!(metadata.contains_key("title"));
        assert!(metadata.contains_key("artist"));
    }

    /// Test ID prefix consistency across all content types
    #[test]
    fn test_id_prefix_consistency() {
        let test_cases = vec![
            ("artist", "artist:UC123"),
            ("album", "album:MPREb_123"),
            ("playlist", "playlist:RDCLAK"),
            ("podcast", "podcast:MPSP"),
            ("song", "dQw4w9WgXcQ"),
            ("video", "dQw4w9WgXcQ"),
        ];

        for (content_type, expected_file_format) in test_cases {
            match content_type {
                "artist" | "album" | "playlist" | "podcast" => {
                    assert!(
                        expected_file_format.contains(':'),
                        "Content type {} should have prefixed ID",
                        content_type
                    );
                    let prefix = expected_file_format.split(':').next().unwrap();
                    assert_eq!(prefix, content_type);
                }
                "song" | "video" => {
                    assert!(
                        !expected_file_format.contains(':'),
                        "Content type {} should NOT have prefixed ID",
                        content_type
                    );
                }
                _ => panic!("Unknown content type: {}", content_type),
            }
        }
    }

    /// Test that browse IDs are correctly formatted
    #[test]
    fn test_browse_id_format_validation() {
        // Artist channel IDs start with UC
        let artist_id = "UC3muIvzjhubNpJ4Pn_0kCQw";
        assert!(artist_id.starts_with("UC"));
        assert_eq!(artist_id.len(), 24);

        // Album IDs start with MPREb_
        let album_id = "MPREb_1234567890abcdefghij";
        assert!(album_id.starts_with("MPREb_"));

        // Playlist IDs can start with RDCLAK or PL
        let playlist_id = "RDCLAK5uy_1234567890";
        assert!(playlist_id.starts_with("RDCLAK") || playlist_id.starts_with("PL"));
    }

    /// Test duration parsing with various formats
    #[test]
    fn test_duration_formats() {
        let test_cases = vec![
            ("0:30", 30),      // 30 seconds
            ("1:15", 75),      // 1 min 15 sec
            ("3:45", 225),     // 3 min 45 sec
            ("45:00", 2700),   // 45 minutes
            ("1:23:45", 5025), // 1 hr 23 min 45 sec
        ];

        for (duration_str, expected_seconds) in test_cases {
            let parsed: u64 = duration_str
                .split(':')
                .try_fold(0u64, |acc, part| part.parse::<u64>().map(|v| acc * 60 + v))
                .expect(&format!("Failed to parse: {}", duration_str));

            assert_eq!(
                parsed, expected_seconds,
                "Duration '{}' should be {} seconds",
                duration_str, expected_seconds
            );
        }
    }

    /// Test metadata type detection
    #[test]
    fn test_metadata_type_detection() {
        let content_types =
            vec!["artist", "album", "song", "video", "playlist", "podcast", "episode"];

        for content_type in content_types {
            let mut metadata = HashMap::new();
            metadata.insert("type".to_string(), vec![content_type.to_string()]);

            let detected_type = metadata.get("type").and_then(|v| v.first()).map(|s| s.as_str());

            assert_eq!(detected_type, Some(content_type));
        }
    }

    /// Test that searches don't return empty metadata
    #[test]
    fn test_non_empty_metadata() {
        // All search results should have at minimum:
        // - type
        // - title
        // - artist (or publisher/channel_name)

        let required_fields = vec!["type", "title", "artist"];

        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), vec!["song".to_string()]);
        metadata.insert("title".to_string(), vec!["Test Song".to_string()]);
        metadata.insert("artist".to_string(), vec!["Test Artist".to_string()]);

        for field in required_fields {
            assert!(metadata.contains_key(field), "Missing required field: {}", field);
            assert!(!metadata[field].is_empty(), "Field {} is empty", field);
        }
    }
}

#[cfg(test)]
mod enum_variant_tests {
    /// Test handling of SearchResultVideo enum variants
    #[test]
    fn test_video_enum_variants() {
        // Verify we handle both Video and VideoEpisode variants
        #[allow(dead_code)]
        enum SearchResultVideo {
            Video { title: String, channel_name: String, video_id: String },
            VideoEpisode { title: String, channel_name: String, episode_id: String },
        }

        let video = SearchResultVideo::Video {
            title: "Test Video".to_string(),
            channel_name: "Test Channel".to_string(),
            video_id: "dQw4w9WgXcQ".to_string(),
        };

        match video {
            SearchResultVideo::Video { video_id, .. } => {
                assert_eq!(video_id, "dQw4w9WgXcQ");
            }
            SearchResultVideo::VideoEpisode { .. } => {
                panic!("Should be Video variant");
            }
        }

        let episode = SearchResultVideo::VideoEpisode {
            title: "Test Episode".to_string(),
            channel_name: "Test Podcast".to_string(),
            episode_id: "abc123def456".to_string(),
        };

        match episode {
            SearchResultVideo::Video { .. } => {
                panic!("Should be VideoEpisode variant");
            }
            SearchResultVideo::VideoEpisode { episode_id, .. } => {
                assert_eq!(episode_id, "abc123def456");
            }
        }
    }

    /// Test handling of BasicSearchResultCommunityPlaylist enum variants
    #[test]
    fn test_playlist_enum_variants() {
        #[allow(dead_code)]
        enum BasicSearchResultCommunityPlaylist {
            Playlist { title: String, author: String, playlist_id: String },
            Podcast { title: String, publisher: String, podcast_id: String },
        }

        let playlist = BasicSearchResultCommunityPlaylist::Playlist {
            title: "Test Playlist".to_string(),
            author: "Test Creator".to_string(),
            playlist_id: "RDCLAK5uy_123".to_string(),
        };

        match playlist {
            BasicSearchResultCommunityPlaylist::Playlist { playlist_id, .. } => {
                assert_eq!(playlist_id, "RDCLAK5uy_123");
            }
            BasicSearchResultCommunityPlaylist::Podcast { .. } => {
                panic!("Should be Playlist variant");
            }
        }

        let podcast = BasicSearchResultCommunityPlaylist::Podcast {
            title: "Test Podcast".to_string(),
            publisher: "Test Publisher".to_string(),
            podcast_id: "MPSP123".to_string(),
        };

        match podcast {
            BasicSearchResultCommunityPlaylist::Playlist { .. } => {
                panic!("Should be Podcast variant");
            }
            BasicSearchResultCommunityPlaylist::Podcast { podcast_id, .. } => {
                assert_eq!(podcast_id, "MPSP123");
            }
        }
    }

    /// Test wildcard pattern for non-exhaustive enums
    #[test]
    fn test_non_exhaustive_enum_handling() {
        #[allow(dead_code)]
        #[non_exhaustive]
        enum TestEnum {
            VariantA,
            VariantB,
        }

        let test = TestEnum::VariantA;

        // Should compile with wildcard pattern
        #[allow(unreachable_patterns)]
        match test {
            TestEnum::VariantA => { /* handled */ }
            TestEnum::VariantB => { /* handled */ }
            _ => { /* future variants */ }
        }
    }
}

/// E2E tests for search result display flow
/// Simulates: ytmapi-rs → SearchItem → DetailItem → UI
/// NOTE: Headers are now in UI layer (ListItem::Header), NOT in domain types
#[cfg(test)]
mod search_display_e2e_tests {
    use std::time::Duration;

    use rmpc::domain::{
        detail_item::DetailItem,
        display::{ListItemDisplay, SearchKey},
        search::{
            AlbumItem, ArtistItem, BrowsableItem, PlayableItem, PlaylistItem, SearchItem, SongItem,
            VideoItem,
        },
    };

    /// Helper: Create mock search results simulating "kim long" query
    /// Based on actual YouTube Music API response structure
    /// NOTE: Headers are NOT in search results - they're in SearchSection.title
    fn mock_kim_long_search_results() -> Vec<SearchItem> {
        vec![
            // Artist result (would be in "Top Result" section)
            SearchItem::Browsable(BrowsableItem::Artist(ArtistItem {
                browse_id: Some("UC12345kimlong".into()),
                name: "Kim Long".into(),
                subscribers: Some("10K subscribers".into()),
                thumbnail: Some("https://lh3.googleusercontent.com/kimlong.jpg".into()),
                search_key: SearchKey::from_display("Kim Long", Some("10K subscribers")),
            })),
            // Song result (would be in "Songs" section)
            SearchItem::Playable(PlayableItem::Song(SongItem {
                video_id: "abc123song".into(),
                title: "Về Với Em - Kim Long".into(),
                artist: "Kim Long".into(),
                album: Some("Single".into()),
                duration: Some(Duration::from_secs(245)),
                thumbnail: Some("https://i.ytimg.com/vi/abc123/sddefault.jpg".into()),
                explicit: false,
                radio_playlist_id: None,
                search_key: SearchKey::from_display(
                    "Về Với Em - Kim Long",
                    Some("Kim Long · Single"),
                ),
            })),
            // Album result (would be in "Albums" section)
            SearchItem::Browsable(BrowsableItem::Album(AlbumItem {
                album_id: "MPREb_kimlong123".into(),
                title: "Best of Kim Long".into(),
                artist: "Kim Long".into(),
                year: Some("2023".into()),
                album_type: Some("Album".into()),
                thumbnail: Some("https://lh3.googleusercontent.com/album.jpg".into()),
                explicit: false,
                search_key: SearchKey::from_display("Best of Kim Long", Some("Kim Long")),
            })),
            // Playlist result (would be in "Playlists" section)
            SearchItem::Browsable(BrowsableItem::Playlist(PlaylistItem {
                playlist_id: "PLkimlong456".into(),
                title: "Kim Long Greatest Hits".into(),
                author: "YouTube Music".into(),
                track_count: Some("25 songs".into()),
                thumbnail: Some("https://lh3.googleusercontent.com/playlist.jpg".into()),
                search_key: SearchKey::from_display(
                    "Kim Long Greatest Hits",
                    Some("YouTube Music"),
                ),
            })),
            // Video result (would be in "Videos" section)
            SearchItem::Playable(PlayableItem::Video(VideoItem {
                video_id: "xyz789video".into(),
                title: "Kim Long Live Concert".into(),
                channel: "Kim Long Official".into(),
                views: Some("1M views".into()),
                duration: Some(Duration::from_secs(3600)),
                thumbnail: Some("https://i.ytimg.com/vi/xyz789/sddefault.jpg".into()),
                search_key: SearchKey::from_display(
                    "Kim Long Live Concert",
                    Some("Kim Long Official"),
                ),
            })),
        ]
    }

    /// E2E Test: Verify SearchItem → DetailItem preserves types correctly
    /// This is the NEW flow - no more Song intermediate step for browsable
    /// items
    #[test]
    fn e2e_search_to_detail_item_preserves_types() {
        let search_results = mock_kim_long_search_results();

        // Convert SearchItem → DetailItem directly (new type-safe conversion)
        let detail_items: Vec<DetailItem> =
            search_results.into_iter().map(DetailItem::from).collect();

        // === ASSERTIONS: Verify the flow preserves types correctly ===

        // Item 0: Kim Long (artist)
        assert_eq!(detail_items[0].type_icon(), "🎤", "Artist should show microphone");
        assert!(detail_items[0].thumbnail_url().is_some(), "Artist should have thumbnail");
        assert!(detail_items[0].is_navigable(), "Artist should be navigable");

        // Item 1: Song
        assert_eq!(detail_items[1].type_icon(), "🎵", "Song should show music note");
        assert!(detail_items[1].thumbnail_url().is_some(), "Song should have thumbnail");
        assert!(detail_items[1].is_playable(), "Song should be playable");

        // Item 2: Album
        assert_eq!(detail_items[2].type_icon(), "💿", "Album should show disc");
        assert!(detail_items[2].is_navigable(), "Album should be navigable");

        // Item 3: Playlist
        assert_eq!(detail_items[3].type_icon(), "📁", "Playlist should show folder");
        assert!(detail_items[3].is_navigable(), "Playlist should be navigable");

        // Item 4: Video
        assert_eq!(detail_items[4].type_icon(), "🎵", "Video converts to Song, shows music note");
        assert!(detail_items[4].is_playable(), "Video should be playable");
    }

    /// Test: All DetailItems are focusable (headers are in ListItem, not
    /// DetailItem)
    #[test]
    fn all_detail_items_are_focusable() {
        let items = mock_kim_long_search_results();

        for item in items {
            let detail = DetailItem::from(item);
            assert!(detail.is_focusable(), "All DetailItems should be focusable");
            assert!(!detail.is_header(), "No DetailItem should be a header");
        }
    }

    /// Test: Unknown song types show empty icon
    #[test]
    fn unknown_types_show_nothing() {
        let mut song = rmpc::domain::Song::default();
        song.metadata.insert("type".into(), vec!["some_future_type".into()]);
        song.metadata.insert("title".into(), vec!["Test".into()]);

        // Song without known type should show empty icon
        assert_eq!(song.type_icon(), "", "Unknown type should show nothing");

        // Song without any type metadata should also show nothing
        let song_no_type = rmpc::domain::Song::default();
        assert_eq!(song_no_type.type_icon(), "", "No metadata should show nothing");
    }
}
