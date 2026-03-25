#![cfg(test)]

use std::sync::OnceLock;

use anyhow::{Result, bail};
use async_trait::async_trait;
use parking_lot::{Mutex, MutexGuard};

use crate::backends::youtube::media::{MediaPreparer, PreparedMedia, PreloadTier};

static MPV_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) type MpvTestGuard = MutexGuard<'static, ()>;

pub(crate) fn acquire_mpv_test_guard() -> MpvTestGuard {
    MPV_TEST_LOCK.get_or_init(|| Mutex::new(())).lock()
}

#[derive(Default)]
pub(crate) struct RecordingMediaPreparer {
    pub(crate) prepared: Mutex<Vec<(String, PreloadTier)>>,
    pub(crate) prefetched: Mutex<Vec<(String, PreloadTier)>>,
    pub(crate) warmed: Mutex<Vec<String>>,
    pub(crate) warmed_batches: Mutex<Vec<Vec<String>>>,
    pub(crate) activated_windows: Mutex<Vec<Vec<String>>>,
    fail_on_prepare: bool,
}

impl RecordingMediaPreparer {
    pub(crate) fn fail_on_prepare() -> Self {
        Self { fail_on_prepare: true, ..Self::default() }
    }
}

#[async_trait]
impl MediaPreparer for RecordingMediaPreparer {
    async fn prepare(&self, track_id: &str, tier: PreloadTier) -> Result<PreparedMedia> {
        if self.fail_on_prepare {
            bail!("prepare should not be called in this test");
        }

        self.prepared.lock().push((track_id.to_string(), tier));
        Ok(PreparedMedia::Direct { url: format!("https://example.invalid/{track_id}") })
    }

    fn prefetch(&self, track_id: &str, tier: PreloadTier) {
        self.prefetched.lock().push((track_id.to_string(), tier));
    }

    fn warm(&self, track_id: &str) {
        self.warmed.lock().push(track_id.to_string());
    }

    fn warm_many(&self, track_ids: &[String]) {
        self.warmed_batches.lock().push(track_ids.to_vec());
    }

    fn activate_playback_window(&self, track_ids: &[String]) {
        self.activated_windows.lock().push(track_ids.to_vec());
    }
}
