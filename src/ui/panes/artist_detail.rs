//! ArtistDetailPane - DetailPane implementation for artist content.
//!
//! Displays artist information with:
//! - Top songs section
//! - Albums section
//! - Singles section
//! - Related artists section
//!
//! Uses ContentView<ArtistContent> for unified stack and key handling.

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    backends::youtube::protocol::play_intent::{ContextSource, PlayIntent},
    ctx::Ctx,
    domain::{ArtistContent, DetailItem, content::ContentType},
    shared::{key_event::KeyEvent, macros::status_info},
    ui::{
        panes::navigator_types::{
            ContentAction, DetailId, DetailPane, EntityContent, EntityRef, InputMode,
            NavigatorPane, PaneAction, PaneId,
        },
        widgets::content_view::ContentView,
    },
};

// =============================================================================
// ARTIST DETAIL PANE
// =============================================================================

/// DetailPane for displaying artist content with stacking.
///
/// Uses ContentView<ArtistContent> for all stack management and key handling.
/// Supports navigating through multiple artists (e.g., Artist A → Related
/// Artist B).
#[derive(Debug, Clone, Default)]
pub struct ArtistDetailPane {
    /// ContentView handles all stack management and key handling
    view: ContentView<ArtistContent>,
}

impl ArtistDetailPane {
    /// Create a new empty ArtistDetailPane.
    pub fn new() -> Self {
        Self { view: ContentView::new() }
    }
}

// =============================================================================
// PANE TRAIT IMPLEMENTATION
// =============================================================================

impl NavigatorPane for ArtistDetailPane {
    fn id(&self) -> PaneId {
        PaneId::Detail(DetailId::Artist)
    }

    fn mode(&self) -> InputMode {
        self.view.mode()
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) -> Result<()> {
        self.view.render(frame, area, ctx);
        Ok(())
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &mut Ctx) -> Result<PaneAction> {
        Ok(match self.view.handle_key(key, ctx) {
            ContentAction::Handled => PaneAction::Handled,
            ContentAction::Back => PaneAction::BackPane,
            ContentAction::Activate(item) => self.resolve_action(item),
            ContentAction::PlayScope(item) => self.resolve_play_scope_action(item, ctx),
            ContentAction::Mark(_) => PaneAction::Handled,
            ContentAction::MoveUp(_) | ContentAction::MoveDown(_) | ContentAction::Delete(_) => {
                PaneAction::Handled
            }
            ContentAction::Enqueue(items) => {
                // 'a' key: Add to queue without playing
                let songs: Vec<_> = items.iter().filter_map(|i| i.as_song()).cloned().collect();
                if !songs.is_empty() { PaneAction::Enqueue(songs) } else { PaneAction::Handled }
            }
            ContentAction::Passthrough => PaneAction::Handled,
        })
    }
}

impl ArtistDetailPane {
    /// Interpret what activation means for a DetailItem in ArtistDetailPane.
    ///
    /// Uses Selection to handle marked-items-vs-current logic uniformly.
    fn resolve_action(&self, item: DetailItem) -> PaneAction {
        match item {
            DetailItem::Song(song) => {
                // Use Selection to get marked items or fall back to current
                let selection = self.view.get_selection();
                let songs = selection.songs_cloned();

                if songs.len() > 1 {
                    // Multiple songs selected - play all starting from activated song
                    let start_index = selection.find_song_index(&song.uri).unwrap_or(0);
                    PaneAction::PlayAll { songs, start_index }
                } else {
                    // Single song - play it
                    PaneAction::Play(song)
                }
            }
            DetailItem::Ref(content_ref) => {
                let entity_type = match content_ref.content_type {
                    ContentType::Artist => DetailId::Artist,
                    ContentType::Album => DetailId::Album,
                    ContentType::Playlist => DetailId::Playlist,
                    _ => return PaneAction::Handled,
                };
                PaneAction::NavigateTo(EntityRef {
                    entity_type,
                    id: content_ref.id,
                    name: content_ref.name,
                })
            }
        }
    }

    fn resolve_play_scope_action(&mut self, item: DetailItem, ctx: &mut Ctx) -> PaneAction {
        if let DetailItem::Ref(content_ref) = &item {
            if matches!(content_ref.content_type, ContentType::Album | ContentType::Playlist) {
                let entity_type = match content_ref.content_type {
                    ContentType::Album => DetailId::Album,
                    ContentType::Playlist => DetailId::Playlist,
                    _ => unreachable!(),
                };
                return PaneAction::PlayRef(EntityRef {
                    entity_type,
                    id: content_ref.id.clone(),
                    name: content_ref.name.clone(),
                });
            }
        }

        let Some(level) = self.view.current() else {
            return PaneAction::Handled;
        };

        let artist_id = level.content.id.clone();
        let artist_name = level.content.name.clone();
        let top_songs = level.content.top_songs.clone();
        let has_marked = level.section_list.has_marked();
        let selection = has_marked.then(|| level.section_list.get_selection());
        let marked_tracks =
            selection.as_ref().map(|selection| selection.songs_cloned()).unwrap_or_default();
        let ignored_marked_refs =
            selection.as_ref().map(|selection| selection.refs().len()).unwrap_or(0);

        if ignored_marked_refs > 0 {
            log::trace!(artist_id = artist_id.as_str(), artist_name = artist_name.as_str(), ignored_marked_refs = ignored_marked_refs; "Artist detail play-scope ignoring non-song marked items");
        }

        let Some((intent, used_marks)) = build_artist_play_scope_intent(
            &item,
            marked_tracks,
            top_songs,
            ctx.status.random,
            ContextSource::Artist { artist_id },
        ) else {
            return PaneAction::Handled;
        };

        ctx.queue_mutator().play(intent);
        if used_marks {
            if let Some(level) = self.view.current_mut() {
                level.section_list.clear_marks();
            }
            status_info!("Play selected songs");
        } else {
            status_info!("Play {}", artist_name);
        }

        PaneAction::Handled
    }
}

fn build_artist_play_scope_intent(
    item: &DetailItem,
    marked_tracks: Vec<crate::domain::Song>,
    top_songs: Vec<crate::domain::Song>,
    shuffle: bool,
    source: ContextSource,
) -> Option<(PlayIntent, bool)> {
    if !marked_tracks.is_empty() {
        return Some((PlayIntent::replace_and_play(marked_tracks, 0, shuffle, Some(source)), true));
    }

    if matches!(item, DetailItem::Ref(content_ref) if matches!(content_ref.content_type, ContentType::Album | ContentType::Playlist))
    {
        return None;
    }

    if top_songs.is_empty() {
        return None;
    }

    Some((PlayIntent::replace_and_play(top_songs, 0, shuffle, Some(source)), false))
}

// =============================================================================
// DETAIL PANE TRAIT IMPLEMENTATION
// =============================================================================

impl DetailPane for ArtistDetailPane {
    fn detail_id(&self) -> DetailId {
        DetailId::Artist
    }

    fn has_content(&self) -> bool {
        self.view.has_content()
    }

    fn stack_depth(&self) -> usize {
        self.view.stack_depth()
    }

    fn push(&mut self, content: EntityContent) {
        if let EntityContent::Artist(artist) = content {
            self.view.push(artist);
        }
    }

    fn pop(&mut self) -> bool {
        self.view.pop()
    }

    fn clear(&mut self) {
        self.view.clear();
    }

    fn current_title(&self) -> Option<&str> {
        self.view.current_title()
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{ContentRef, Song, content::Extensions},
        tests::fixtures,
    };

    fn make_song(uri: &str, title: &str) -> Song {
        let mut song = Song::default();
        song.uri = uri.to_string();
        song.metadata.insert("title".to_string(), vec![title.to_string()]);
        song
    }

    fn make_test_artist() -> ArtistContent {
        ArtistContent {
            id: "artist123".to_string(),
            name: "Test Artist".to_string(),
            top_songs: vec![],
            thumbnail: None,
            bio: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn test_artist_pane_creation() {
        let pane = ArtistDetailPane::new();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_artist_pane_push_pop() {
        let mut pane = ArtistDetailPane::new();

        // Push first artist
        pane.push(EntityContent::Artist(make_test_artist()));
        assert!(pane.has_content());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Artist"));

        // Push second artist
        let mut artist2 = make_test_artist();
        artist2.name = "Artist 2".to_string();
        pane.push(EntityContent::Artist(artist2));
        assert_eq!(pane.stack_depth(), 2);
        assert_eq!(pane.current_title(), Some("Artist 2"));

        // Pop should go back to first
        assert!(pane.pop());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Artist"));

        // Pop at depth 1 should fail
        assert!(!pane.pop());
        assert_eq!(pane.stack_depth(), 1);
    }

    #[test]
    fn test_artist_pane_clear() {
        let mut pane = ArtistDetailPane::new();
        pane.push(EntityContent::Artist(make_test_artist()));
        pane.push(EntityContent::Artist(make_test_artist()));

        pane.clear();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_pane_id() {
        let pane = ArtistDetailPane::new();
        assert_eq!(pane.id(), PaneId::Detail(DetailId::Artist));
        assert_eq!(pane.detail_id(), DetailId::Artist);
    }

    #[test]
    fn play_scope_intent_uses_marked_tracks_before_top_songs() {
        let item = DetailItem::song(make_song("song-2", "Song 2"));
        let marked = vec![make_song("song-2", "Song 2"), make_song("song-3", "Song 3")];
        let top_songs = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];

        let (intent, used_marks) = build_artist_play_scope_intent(
            &item,
            marked.clone(),
            top_songs,
            true,
            ContextSource::Artist { artist_id: "artist123".into() },
        )
        .expect("marked songs should build an artist play-scope intent");

        assert!(used_marks);
        assert!(matches!(
            intent,
            PlayIntent::Replace(replace)
                if matches!(replace.playback, crate::backends::youtube::protocol::play_intent::ReplacePlayback::StartAtIndex(0))
                    && matches!(&replace.source, Some(ContextSource::Artist { artist_id }) if artist_id == "artist123")
                    && replace.tracks.iter().map(|song| song.uri.clone()).collect::<Vec<_>>() == marked.iter().map(|song| song.uri.clone()).collect::<Vec<_>>()
        ));
    }

    #[test]
    fn play_scope_marks_clear_after_successful_marked_play() {
        let mut pane = ArtistDetailPane::new();
        let mut artist = make_test_artist();
        artist.top_songs = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];
        artist.extensions = Extensions::builder()
            .related_playlists("Featured", vec![ContentRef::playlist("playlist-1", "Mix 1")])
            .build();
        pane.push(EntityContent::Artist(artist));

        let selected_item = {
            let level = pane.view.current_mut().expect("artist content should exist");
            level.section_list.select_first();
            level.section_list.list_view_mut().toggle_mark();
            while !matches!(
                level.section_list.selected_item(),
                Some(DetailItem::Ref(content_ref)) if content_ref.content_type == ContentType::Playlist
            ) {
                level.section_list.select_next();
            }
            level.section_list.list_view_mut().toggle_mark();
            level.section_list.select_first();
            level.section_list.selected_item().cloned().expect("song should be selected")
        };

        let mut ctx = fixtures::ctx();
        ctx.status.random = true;

        assert!(matches!(
            pane.resolve_play_scope_action(selected_item, &mut ctx),
            PaneAction::Handled
        ));
        let queue = ctx.queue_state().read();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].uri, "song-1");
        drop(queue);
        assert!(
            !pane.view.current().expect("artist content should exist").section_list.has_marked()
        );
    }

    #[test]
    fn play_scope_without_marks_plays_top_songs_from_start() {
        let mut pane = ArtistDetailPane::new();
        let mut artist = make_test_artist();
        artist.top_songs = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];
        pane.push(EntityContent::Artist(artist));

        let selected_item = {
            let level = pane.view.current_mut().expect("artist content should exist");
            level.section_list.select_first();
            level.section_list.selected_item().cloned().expect("song should be selected")
        };

        let mut ctx = fixtures::ctx();
        ctx.status.random = false;

        assert!(matches!(
            pane.resolve_play_scope_action(selected_item, &mut ctx),
            PaneAction::Handled
        ));
        let queue = ctx.queue_state().read();
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].uri, "song-1");
        assert_eq!(queue[1].uri, "song-2");
        assert_eq!(queue[2].uri, "song-3");
    }

    #[test]
    fn play_scope_defers_album_playlist_refs_to_direct_play_bead() {
        let item = DetailItem::playlist("playlist-1", "Mix 1");
        let top_songs = vec![make_song("song-1", "Song 1")];

        let intent = build_artist_play_scope_intent(
            &item,
            vec![],
            top_songs,
            false,
            ContextSource::Artist { artist_id: "artist123".into() },
        );

        assert!(intent.is_none());
    }
}
