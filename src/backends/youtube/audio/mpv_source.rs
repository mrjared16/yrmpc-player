use anyhow::Result;

#[derive(Debug, Clone)]
pub struct MpvInput {
    pub url: String,
    pub mpv_args: Vec<String>,
}

impl MpvInput {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            mpv_args: Vec::new(),
        }
    }

    pub fn with_args(url: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            url: url.into(),
            mpv_args: args,
        }
    }
}

pub trait MpvAudioSource: Send {
    fn startup(&mut self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&mut self) {}

    fn build_mpv_input(&mut self, video_id: &str) -> Result<MpvInput>;
}
