//! Database management operations (MPD only)
//!
//! Update and rescan the music database.

use anyhow::Result;

use crate::backends::mpd::specific::Database;

/// Manages the music database (MPD only)
///
/// # Example
///
/// ```ignore
/// if let Some(db) = dispatcher.database() {
///     // Update the entire database
///     let job_id = db.update(None)?;
///     
///     // Update a specific path
///     db.update(Some("Music/NewAlbum"))?;
/// }
/// ```
pub struct DatabaseController<'a> {
    pub(crate) backend: &'a mut dyn Database,
}

impl DatabaseController<'_> {
    /// Update the database
    ///
    /// Scans for new files and updates metadata for changed files.
    ///
    /// # Arguments
    /// * `path` - Optional path to update (None = entire database)
    ///
    /// # Returns
    /// Job ID for tracking the update progress
    pub fn update(&mut self, path: Option<&str>) -> Result<u32> {
        self.backend.update(path)
    }

    /// Rescan the database
    ///
    /// Like update, but also rescans unchanged files.
    /// Useful when metadata has been modified externally.
    ///
    /// # Arguments
    /// * `path` - Optional path to rescan (None = entire database)
    ///
    /// # Returns
    /// Job ID for tracking the rescan progress
    pub fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        self.backend.rescan(path)
    }
}
