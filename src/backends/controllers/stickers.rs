//! Sticker metadata operations (MPD only)
//!
//! Stickers are arbitrary key-value metadata that can be attached to songs.
//! Commonly used for ratings, play counts, etc.

use anyhow::Result;
use std::collections::HashMap;

use crate::backends::mpd::specific::Stickers;

/// Manages sticker metadata (MPD only)
///
/// Stickers are key-value pairs attached to songs, useful for
/// ratings, play counts, and other custom metadata.
///
/// # Example
///
/// ```ignore
/// if let Some(stickers) = dispatcher.stickers() {
///     // Set a rating
///     stickers.set("song.mp3", "rating", "5")?;
///     
///     // Get a rating
///     if let Some(rating) = stickers.get("song.mp3", "rating")? {
///         println!("Rating: {}", rating);
///     }
/// }
/// ```
pub struct StickerController<'a> {
    pub(crate) backend: &'a mut dyn Stickers,
}

impl StickerController<'_> {
    /// Get a sticker value
    ///
    /// Returns `None` if the sticker doesn't exist.
    pub fn get(&mut self, uri: &str, key: &str) -> Result<Option<String>> {
        self.backend.get(uri, key)
    }

    /// Set a sticker value
    pub fn set(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.backend.set(uri, key, value)
    }

    /// Delete a sticker
    pub fn delete(&mut self, uri: &str, key: &str) -> Result<()> {
        self.backend.delete(uri, key)
    }

    /// List all stickers for a song
    pub fn list(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.backend.list(uri)
    }

    /// Check if a song has a specific sticker
    pub fn has(&mut self, uri: &str, key: &str) -> Result<bool> {
        let stickers = self.backend.list(uri)?;
        Ok(stickers.contains_key(key))
    }
    
    /// Delete all stickers for a song
    pub fn delete_all(&mut self, uri: &str) -> Result<()> {
        self.backend.delete_all(uri)
    }
}
