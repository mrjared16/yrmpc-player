pub mod video_id {
    pub const ON_TAP: &str = "/onTap/watchEndpoint/videoId";
    pub const TITLE_NAV: &str = "/title/runs/0/navigationEndpoint/watchEndpoint/videoId";
    pub const PLAYLIST_ITEM: &str = "/playlistItemData/videoId";
    pub const ALL: &[&str] = &[ON_TAP, TITLE_NAV, PLAYLIST_ITEM];
}

pub mod browse_id {
    pub const NAV_ENDPOINT: &str = "/navigationEndpoint/browseEndpoint/browseId";
    pub const TITLE_NAV: &str = "/title/runs/0/navigationEndpoint/browseEndpoint/browseId";
    pub const ALL: &[&str] = &[NAV_ENDPOINT, TITLE_NAV];
}

pub mod result_type {
    pub const SUBTITLE_TEXT: &str = "/subtitle/runs/0/text";
    pub const SUBTITLE_SIMPLE: &str = "/subtitle/simpleText";
    pub const ALL: &[&str] = &[SUBTITLE_TEXT, SUBTITLE_SIMPLE];
}

pub mod title {
    pub const TITLE_TEXT: &str = "/title/runs/0/text";
    pub const TITLE_SIMPLE: &str = "/title/simpleText";
    pub const ALL: &[&str] = &[TITLE_TEXT, TITLE_SIMPLE];
}

pub mod thumbnails {
    pub const STANDARD: &str = "/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails";
    pub const CARD: &str = "/thumbnail/thumbnails";
    pub const ALL: &[&str] = &[STANDARD, CARD];
}
