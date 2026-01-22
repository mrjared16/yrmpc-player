//! Stream resolution trait

use std::time::SystemTime;

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, Default)]
pub struct AudioFormat {
    pub mime_type: Option<String>,
    pub bitrate: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub url: String,
    pub expires_at: Option<SystemTime>,
    pub format: AudioFormat,
}

#[async_trait]
pub trait StreamResolver: Send + Sync {
    async fn resolve(&self, track_id: &str) -> Result<StreamInfo>;
    fn is_cached(&self, track_id: &str) -> bool;
    fn invalidate(&self, track_id: &str);
}
