//! PlayHandler - handles Play action for songs/albums/playlists.
//!
//! ## Responsibility (SRP)
//!
//! This handler handles PLAYING new content:
//! - Clear queue, add songs, start playback
//!
//! It does NOT handle:
//! - Toggle play/pause (that's TogglePlaybackHandler)

use anyhow::Result;

use crate::{
    actions::{
        intent::{IntentKind, Intent, Selection},
        handler::{HandleResult, Handler},
    },
    ctx::Ctx,
    domain::ContentType,
};

/// Handles Play actions.
#[derive(Debug, Default)]
pub struct PlayHandler;

impl PlayHandler {
    /// Create a new PlayHandler.
    pub fn new() -> Self {
        Self
    }

    /// Validate selection for play action.
    fn validate(&self, selection: &Selection) -> Result<(), &'static str> {
        if selection.is_empty() {
            return Err("Select something to play");
        }

        if !selection.has_only(&[ContentType::Track, ContentType::Album, ContentType::Playlist]) {
            return Err("Cannot play this content type");
        }

        if !selection.is_homogeneous() {
            return Err("Select only one content type to play");
        }

        Ok(())
    }
}

impl Handler for PlayHandler {
    fn execute(&self, intent: &Intent, ctx: &mut Ctx) -> Result<HandleResult> {
        // Only handle Play action
        if intent.action != IntentKind::Play {
            return Ok(HandleResult::Skip);
        }

        // Validate
        if let Err(reason) = self.validate(&intent.selection) {
            return Ok(HandleResult::NotApplicable(reason));
        }

        let songs = intent.selection.songs_cloned();

        if songs.is_empty() {
            // TODO: Handle albums/playlists by expanding to songs
            return Ok(HandleResult::NotApplicable(
                "Album/playlist expansion not yet implemented",
            ));
        }

        // Play the song(s)
        // For now, play the first song directly
        if let Some(first_song) = songs.first() {
            if let Some(id) = first_song.id {
                ctx.command(move |client| {
                    client.play_id(id)?;
                    Ok(())
                });
            }
        }

        Ok(HandleResult::Done)
    }

    fn priority(&self) -> i32 {
        0
    }

    fn name(&self) -> &'static str {
        "PlayHandler"
    }
}
