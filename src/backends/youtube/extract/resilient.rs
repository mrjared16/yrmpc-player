use serde_json::Value;
use crate::backends::youtube::extract::{extract_string, paths};

pub struct ResilientExtractor<'a> {
    json: &'a Value,
}

impl<'a> ResilientExtractor<'a> {
    pub fn new(json: &'a Value) -> Self {
        Self { json }
    }

    pub fn video_id(&self) -> Option<String> {
        extract_string(self.json, paths::video_id::ALL)
    }

    pub fn browse_id(&self) -> Option<String> {
        extract_string(self.json, paths::browse_id::ALL)
    }

    pub fn result_type(&self) -> Option<String> {
        extract_string(self.json, paths::result_type::ALL)
    }

    pub fn title(&self) -> Option<String> {
        extract_string(self.json, paths::title::ALL)
    }

    pub fn try_extract_top_result(&self) -> ExtractedTopResult {
        ExtractedTopResult {
            video_id: self.video_id(),
            title: self.title(),
            result_type: self.result_type(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ExtractedTopResult {
    pub video_id: Option<String>,
    pub title: Option<String>,
    pub result_type: Option<String>,
}

impl ExtractedTopResult {
    pub fn has_video_id(&self) -> bool {
        self.video_id.is_some()
    }
}
