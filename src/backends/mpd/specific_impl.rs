//! MPD-specific trait implementations.
//!
//! This module implements the MPD-specific traits defined in `specific.rs`.

use std::collections::HashMap;

use anyhow::Result;

use super::{
    MpdBackend,
    specific::{Database, Outputs, Stickers},
};
use crate::mpd::{commands::Output, mpd_client::MpdClient as MpdClientTrait};

// =============================================================================
// STICKERS
// =============================================================================

impl<'name> Stickers for MpdBackend<'name> {
    fn list(&mut self, uri: &str) -> Result<HashMap<String, String>> {
        self.client.list_stickers(uri).map(|s| s.0).map_err(Into::into)
    }

    fn set(&mut self, uri: &str, key: &str, value: &str) -> Result<()> {
        self.client.set_sticker(uri, key, value).map_err(Into::into)
    }

    fn delete(&mut self, uri: &str, key: &str) -> Result<()> {
        self.client.delete_sticker(uri, key).map_err(Into::into)
    }
}

// =============================================================================
// OUTPUTS
// =============================================================================

impl<'name> Outputs for MpdBackend<'name> {
    fn list(&mut self) -> Result<Vec<Output>> {
        self.client.outputs().map(|o| o.0).map_err(Into::into)
    }

    fn enable(&mut self, id: u32) -> Result<()> {
        self.client.enable_output(id).map_err(Into::into)
    }

    fn disable(&mut self, id: u32) -> Result<()> {
        self.client.disable_output(id).map_err(Into::into)
    }

    fn toggle(&mut self, id: u32) -> Result<()> {
        self.client.toggle_output(id).map_err(Into::into)
    }
}

// =============================================================================
// DATABASE
// =============================================================================

impl<'name> Database for MpdBackend<'name> {
    fn update(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.update(path).map(|u| u.job_id).map_err(Into::into)
    }

    fn rescan(&mut self, path: Option<&str>) -> Result<u32> {
        self.client.rescan(path).map(|u| u.job_id).map_err(Into::into)
    }
}
