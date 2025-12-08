use std::time::Duration;

use anyhow::Context;

use crate::mpd::{FromMpd, LineHandled, errors::MpdError};

#[derive(Debug, Default)]
pub struct Count {
    pub songs: usize,
    pub playtime: Duration,
}

impl FromMpd for Count {
    fn next_internal(&mut self, key: &str, value: String) -> Result<LineHandled, MpdError> {
        match key {
            "songs" => {
                self.songs = value.parse().context("failed to parse songs count")?;
            }
            "playtime" => {
                let seconds: u64 = value.parse().context("failed to parse playtime")?;
                self.playtime = Duration::from_secs(seconds);
            }
            _ => return Ok(LineHandled::No { value }),
        }
        Ok(LineHandled::Yes)
    }
}
