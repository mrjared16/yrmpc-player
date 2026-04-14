use std::path::PathBuf;

pub const APP_CONFIG_DIR_NAME: &str = env!("CARGO_CRATE_NAME");
pub const LEGACY_CONFIG_DIR_NAME: &str = "yrmpc";
pub const YOUTUBE_CONFIG_FILE_NAME: &str = "youtube.toml";
pub const COOKIE_FILE_NAME: &str = "cookie.txt";
pub const LEGACY_COOKIE_FILE_NAME: &str = "cookies.txt";

fn config_home_dir() -> Option<PathBuf> {
    dirs::config_dir().or_else(|| dirs::home_dir().map(|home| home.join(".config")))
}

pub fn app_config_dir() -> Option<PathBuf> {
    config_home_dir().map(|dir| dir.join(APP_CONFIG_DIR_NAME))
}

pub fn legacy_config_dir() -> Option<PathBuf> {
    config_home_dir().map(|dir| dir.join(LEGACY_CONFIG_DIR_NAME))
}

pub fn preferred_youtube_config_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join(YOUTUBE_CONFIG_FILE_NAME))
}

pub fn youtube_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = preferred_youtube_config_path() {
        paths.push(path);
    }

    if let Some(path) = legacy_config_dir().map(|dir| dir.join(YOUTUBE_CONFIG_FILE_NAME)) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    paths
}

pub fn preferred_cookie_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join(COOKIE_FILE_NAME))
}

pub fn cookie_file_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = preferred_cookie_path() {
        paths.push(path);
    }

    if let Some(path) = legacy_config_dir().map(|dir| dir.join(LEGACY_COOKIE_FILE_NAME)) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    paths
}
