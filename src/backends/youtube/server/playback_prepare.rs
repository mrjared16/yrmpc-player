use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::runtime::{Builder, Handle};

use crate::backends::youtube::{
    audio::MpvInput,
    media::{MediaPreparer, PreparedMedia},
    services::PlaybackService,
    services::playback_service::RuntimeInputDecision,
};

pub(crate) fn prepare_media_blocking(
    media_preparer: &Arc<dyn MediaPreparer>,
    track_id: &str,
    tier: crate::backends::youtube::media::PreloadTier,
) -> Result<PreparedMedia> {
    if let Ok(handle) = Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(media_preparer.prepare(track_id, tier)))
    } else {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to build Tokio runtime for media preparation")?;
        runtime.block_on(media_preparer.prepare(track_id, tier))
    }
}

pub(crate) fn build_runtime_mpv_input(
    playback: &Arc<PlaybackService>,
    track_id: &str,
    prepared: &PreparedMedia,
) -> Result<MpvInput> {
    playback.build_runtime_input(track_id, prepared)
}

pub(crate) fn build_current_runtime_input_with_direct_fallback(
    playback: &Arc<PlaybackService>,
    track_id: &str,
    prepared: &PreparedMedia,
) -> Result<RuntimeInputDecision> {
    playback.build_current_runtime_input_with_direct_fallback(track_id, prepared)
}
