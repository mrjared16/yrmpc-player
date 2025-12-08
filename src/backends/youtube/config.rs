//! YouTube backend configuration

use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

/// Stream URL extractor type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExtractorType {
    /// yt-dlp CLI (reliable, widely used, ~3-4s per extraction)
    #[default]
    YtDlp,
    /// ytx Go binary (fast, ~200ms, requires ytx in PATH)
    Ytx,
}

/// YouTube backend configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct YouTubeConfig {
    /// Socket path for daemon
    pub socket_path: PathBuf,

    /// Connection timeout
    #[serde(with = "humantime_serde")]
    pub connection_timeout: Duration,

    /// Read timeout
    #[serde(with = "humantime_serde")]
    pub read_timeout: Duration,

    /// Daemon configuration
    pub daemon: DaemonConfig,

    /// MPV configuration
    pub mpv: MpvConfig,

    /// API configuration
    pub api: ApiConfig,
}

/// Daemon configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Auto-start daemon if not running
    pub auto_start: bool,
    
    /// Maximum connection retries
    pub max_retries: u32,
}

/// MPV configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MpvConfig {
    /// Extra MPV arguments
    pub extra_args: Vec<String>,
    
    /// Default volume (0-100)
    pub volume: u8,
    
    /// Audio device
    pub audio_device: Option<String>,
}

/// API configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Cookie file path
    pub cookie_file: Option<PathBuf>,

    /// Cache duration
    #[serde(with = "humantime_serde")]
    pub cache_duration: Duration,

    /// Maximum search results
    pub max_search_results: usize,

    /// Stream URL extractor type
    /// - "ytdlp" (default): Uses yt-dlp CLI, reliable and widely used
    /// - "pytubefix": Uses pytubefix Python library, faster but requires installation
    pub extractor: ExtractorType,
}

impl Default for YouTubeConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/tmp/yrmpc-yt.sock"),
            connection_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
            daemon: DaemonConfig::default(),
            mpv: MpvConfig::default(),
            api: ApiConfig::default(),
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            auto_start: true,
            max_retries: 3,
        }
    }
}

impl Default for MpvConfig {
    fn default() -> Self {
        Self {
            extra_args: vec![
                "--gapless-audio=yes".to_string(),
                "--prefetch-playlist=yes".to_string(),
            ],
            volume: 80,
            audio_device: None,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            cookie_file: None,
            cache_duration: Duration::from_secs(3600),
            max_search_results: 50,
            extractor: ExtractorType::default(), // yt-dlp by default
        }
    }
}

impl YouTubeConfig {
    /// Load configuration from TOML file
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Load from default location (~/.config/yrmpc/youtube.toml)
    pub fn load() -> anyhow::Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("No config directory found"))?;
        let path = config_dir.join("yrmpc/youtube.toml");
        
        if path.exists() {
            Self::from_file(&path)
        } else {
            Ok(Self::default())
        }
    }
}
