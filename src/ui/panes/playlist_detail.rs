//! PlaylistDetailPane - DetailPane implementation for playlist content.
//!
//! Displays playlist information with:
//! - Playlist tracks section
//! - Related playlists section (if available)
//!
//! Uses ContentView<PlaylistContent> for unified stack and key handling.

use anyhow::Result;
use ratatui::{Frame, prelude::Rect};

use crate::{
    backends::youtube::protocol::play_intent::{ContextSource, PlayIntent},
    ctx::Ctx,
    domain::{DetailItem, PlaylistContent, content::ContentType},
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
// PLAYLIST DETAIL PANE
// =============================================================================

/// DetailPane for displaying playlist content with stacking.
///
/// Uses ContentView<PlaylistContent> for all stack management and key handling.
#[derive(Debug, Clone, Default)]
pub struct PlaylistDetailPane {
    /// ContentView handles all stack management and key handling
    view: ContentView<PlaylistContent>,
}

impl PlaylistDetailPane {
    /// Create a new empty PlaylistDetailPane.
    pub fn new() -> Self {
        Self { view: ContentView::new() }
    }
}

// =============================================================================
// PANE TRAIT IMPLEMENTATION
// =============================================================================

impl NavigatorPane for PlaylistDetailPane {
    fn id(&self) -> PaneId {
        PaneId::Detail(DetailId::Playlist)
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
            ContentAction::Activate(item) => self.resolve_action(item, ctx),
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

impl PlaylistDetailPane {
    /// Interpret what activation means for a DetailItem in PlaylistDetailPane.
    ///
    /// Uses Selection to handle marked-items-vs-current logic uniformly.
    fn resolve_action(&self, item: DetailItem, ctx: &Ctx) -> PaneAction {
        match item {
            DetailItem::Song(song) => {
                // Use Selection to get marked items or fall back to current
                let selection = self.view.get_selection();
                let songs = selection.songs_cloned();

                // Get the playlist ID from current content
                let playlist_id =
                    self.view.current().map(|level| level.content.id.clone()).unwrap_or_default();

                if songs.len() > 1 {
                    // Multiple songs selected - play all starting from activated song
                    let start_index = selection.find_song_index(&song.uri).unwrap_or(0);
                    ctx.queue_store().play(PlayIntent::Context {
                        tracks: songs,
                        offset: start_index,
                        shuffle: false,
                        source: Some(ContextSource::Playlist { playlist_id }),
                    });
                } else {
                    // Single song - play it
                    ctx.queue_store().play(PlayIntent::Context {
                        tracks: vec![song],
                        offset: 0,
                        shuffle: false,
                        source: Some(ContextSource::Playlist { playlist_id }),
                    });
                }
                PaneAction::Handled
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

        let playlist_id = level.content.id.clone();
        let playlist_title = level.content.title.clone();
        let full_tracks = level.content.tracks.clone();
        let has_marked = level.section_list.has_marked();
        let marked_tracks =
            if has_marked { level.section_list.get_selection().songs_cloned() } else { vec![] };

        let Some((intent, used_marks)) = build_detail_play_scope_intent(
            &item,
            marked_tracks,
            full_tracks,
            ctx.status.random,
            ContextSource::Playlist { playlist_id },
        ) else {
            return PaneAction::Handled;
        };

        ctx.queue_store().play(intent);
        if used_marks {
            if let Some(level) = self.view.current_mut() {
                level.section_list.clear_marks();
            }
            status_info!("Play selected songs");
        } else {
            status_info!("Play {}", playlist_title);
        }

        PaneAction::Handled
    }
}

fn build_detail_play_scope_intent(
    item: &DetailItem,
    marked_tracks: Vec<crate::domain::Song>,
    full_tracks: Vec<crate::domain::Song>,
    shuffle: bool,
    source: ContextSource,
) -> Option<(PlayIntent, bool)> {
    if !marked_tracks.is_empty() {
        return Some((
            PlayIntent::Context { tracks: marked_tracks, offset: 0, shuffle, source: Some(source) },
            true,
        ));
    }

    if matches!(item, DetailItem::Ref(content_ref) if matches!(content_ref.content_type, ContentType::Album | ContentType::Playlist))
    {
        return None;
    }

    if full_tracks.is_empty() {
        return None;
    }

    Some((
        PlayIntent::Context { tracks: full_tracks, offset: 0, shuffle, source: Some(source) },
        false,
    ))
}

// =============================================================================
// DETAIL PANE TRAIT IMPLEMENTATION
// =============================================================================

impl DetailPane for PlaylistDetailPane {
    fn detail_id(&self) -> DetailId {
        DetailId::Playlist
    }

    fn has_content(&self) -> bool {
        self.view.has_content()
    }

    fn stack_depth(&self) -> usize {
        self.view.stack_depth()
    }

    fn push(&mut self, content: EntityContent) {
        if let EntityContent::Playlist(playlist) = content {
            self.view.push(playlist);
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
        domain::{Song, content::Extensions},
        tests::fixtures,
    };

    fn make_test_playlist() -> PlaylistContent {
        PlaylistContent {
            id: "playlist123".to_string(),
            title: "Test Playlist".to_string(),
            tracks: vec![],
            author: None,
            thumbnail: None,
            description: None,
            track_count: None,
            duration_text: None,
            extensions: Extensions::default(),
        }
    }

    fn make_song(uri: &str, title: &str) -> Song {
        let mut song = Song::default();
        song.uri = uri.to_string();
        song.metadata.insert("title".to_string(), vec![title.to_string()]);
        song
    }

    #[test]
    fn test_playlist_pane_creation() {
        let pane = PlaylistDetailPane::new();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_playlist_pane_push_pop() {
        let mut pane = PlaylistDetailPane::new();

        // Push first playlist
        pane.push(EntityContent::Playlist(make_test_playlist()));
        assert!(pane.has_content());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Playlist"));

        // Push second playlist
        let mut playlist2 = make_test_playlist();
        playlist2.title = "Playlist 2".to_string();
        pane.push(EntityContent::Playlist(playlist2));
        assert_eq!(pane.stack_depth(), 2);
        assert_eq!(pane.current_title(), Some("Playlist 2"));

        // Pop should go back to first
        assert!(pane.pop());
        assert_eq!(pane.stack_depth(), 1);
        assert_eq!(pane.current_title(), Some("Test Playlist"));

        // Pop at depth 1 should fail
        assert!(!pane.pop());
        assert_eq!(pane.stack_depth(), 1);
    }

    #[test]
    fn test_playlist_pane_clear() {
        let mut pane = PlaylistDetailPane::new();
        pane.push(EntityContent::Playlist(make_test_playlist()));
        pane.push(EntityContent::Playlist(make_test_playlist()));

        pane.clear();
        assert!(!pane.has_content());
        assert_eq!(pane.stack_depth(), 0);
    }

    #[test]
    fn test_pane_id() {
        let pane = PlaylistDetailPane::new();
        assert_eq!(pane.id(), PaneId::Detail(DetailId::Playlist));
        assert_eq!(pane.detail_id(), DetailId::Playlist);
    }

    #[test]
    fn play_scope_intent_uses_marked_tracks_before_full_playlist() {
        let item = DetailItem::Song(make_song("song-2", "Song 2"));
        let marked = vec![make_song("song-1", "Song 1"), make_song("song-2", "Song 2")];
        let full = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];

        let (intent, used_marks) = build_detail_play_scope_intent(
            &item,
            marked.clone(),
            full,
            true,
            ContextSource::Playlist { playlist_id: "playlist123".into() },
        )
        .expect("marked tracks should build a play-scope intent");

        assert!(used_marks);
        assert!(matches!(
            intent,
            PlayIntent::Context {
                tracks,
                offset: 0,
                shuffle: true,
                source: Some(ContextSource::Playlist { playlist_id })
            } if playlist_id == "playlist123" && tracks.iter().map(|song| song.uri.clone()).collect::<Vec<_>>() == marked.iter().map(|song| song.uri.clone()).collect::<Vec<_>>()
        ));
    }

    #[test]
    fn play_scope_marks_clear_after_successful_marked_play() {
        let mut pane = PlaylistDetailPane::new();
        let mut playlist = make_test_playlist();
        playlist.tracks = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];
        pane.push(EntityContent::Playlist(playlist));

        let selected_item = {
            let level = pane.view.current_mut().expect("playlist content should exist");
            level.section_list.select_first();
            level.section_list.list_view_mut().toggle_mark();
            level.section_list.select_next();
            level.section_list.list_view_mut().toggle_mark();
            level.section_list.selected_item().cloned().expect("song should be selected")
        };

        let mut ctx = fixtures::ctx();
        ctx.status.random = true;

        assert!(matches!(
            pane.resolve_play_scope_action(selected_item, &mut ctx),
            PaneAction::Handled
        ));
        let queue = ctx.queue_store().read();
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].uri, "song-1");
        assert_eq!(queue[1].uri, "song-2");
        drop(queue);
        assert!(
            !pane.view.current().expect("playlist content should exist").section_list.has_marked()
        );
    }

    #[test]
    fn play_scope_without_marks_plays_full_playlist_from_start() {
        let mut pane = PlaylistDetailPane::new();
        let mut playlist = make_test_playlist();
        playlist.tracks = vec![
            make_song("song-1", "Song 1"),
            make_song("song-2", "Song 2"),
            make_song("song-3", "Song 3"),
        ];
        pane.push(EntityContent::Playlist(playlist));

        let selected_item = pane
            .view
            .current()
            .expect("playlist content should exist")
            .section_list
            .selected_item()
            .cloned()
            .expect("song should be selected");

        let mut ctx = fixtures::ctx();
        ctx.status.random = true;

        assert!(matches!(
            pane.resolve_play_scope_action(selected_item, &mut ctx),
            PaneAction::Handled
        ));
        let queue = ctx.queue_store().read();
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].uri, "song-1");
        assert_eq!(queue[1].uri, "song-2");
        assert_eq!(queue[2].uri, "song-3");
    }
}
