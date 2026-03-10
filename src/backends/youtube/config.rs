//! YouTube backend configuration

use std::{path::PathBuf, time::Duration};

use serde::Deserialize;

/// Stream URL extractor type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExtractorType {
    /// yt-dlp CLI (reliable, widely used, ~3-4s per extraction)
    YtDlp,
    /// ytx Go binary (fast, ~200ms, requires ytx in PATH)
    #[default]
    Ytx,
}

/// Audio delivery mode for playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum AudioDeliveryMode {
    #[serde(rename = "combined", alias = "ffmpegconcat", alias = "concat")]
    #[default]
    Combined,
    #[serde(rename = "direct", alias = "passthrough")]
    Direct,
    #[serde(rename = "relay", alias = "proxy")]
    Relay,
}

impl AudioDeliveryMode {
    pub fn is_direct_like(self) -> bool {
        matches!(self, Self::Direct | Self::Relay)
    }
}

/// Audio streaming configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio delivery mode
    pub source: AudioDeliveryMode,
    /// Cache directory for audio prefixes
    pub cache_dir: Option<PathBuf>,
    /// Prefix size in bytes (default: 200KB)
    pub prefix_size: u64,
    /// Maximum cache size in bytes (default: 200MB)
    pub max_cache_size: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            source: AudioDeliveryMode::default(),
            cache_dir: None,
            prefix_size: 204_800,
            max_cache_size: 209_715_200,
        }
    }
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

    /// Audio streaming configuration
    pub audio: AudioConfig,
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
    /// - "pytubefix": Uses pytubefix Python library, faster but requires
    ///   installation
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
            audio: AudioConfig::default(),
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { auto_start: true, max_retries: 3 }
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
            extractor: ExtractorType::default(), // ytx by default (fast, ~200ms)
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
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("No config directory found"))?;
        let path = config_dir.join("yrmpc/youtube.toml");

        if path.exists() { Self::from_file(&path) } else { Ok(Self::default()) }
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioConfig, AudioDeliveryMode};

    #[test]
    fn audio_delivery_mode_accepts_current_values() {
        let direct: AudioConfig = toml::from_str("source = \"direct\"").expect("parse direct");
        let combined: AudioConfig =
            toml::from_str("source = \"combined\"").expect("parse combined");
        let relay: AudioConfig = toml::from_str("source = \"relay\"").expect("parse relay");

        assert_eq!(direct.source, AudioDeliveryMode::Direct);
        assert_eq!(combined.source, AudioDeliveryMode::Combined);
        assert_eq!(relay.source, AudioDeliveryMode::Relay);
    }

    #[test]
    fn audio_delivery_mode_accepts_legacy_aliases() {
        let passthrough: AudioConfig =
            toml::from_str("source = \"passthrough\"").expect("parse passthrough alias");
        let ffmpegconcat: AudioConfig =
            toml::from_str("source = \"ffmpegconcat\"").expect("parse ffmpegconcat alias");
        let concat: AudioConfig = toml::from_str("source = \"concat\"").expect("parse concat");

        assert_eq!(passthrough.source, AudioDeliveryMode::Direct);
        assert_eq!(ffmpegconcat.source, AudioDeliveryMode::Combined);
        assert_eq!(concat.source, AudioDeliveryMode::Combined);
    }
}
