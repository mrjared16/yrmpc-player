//! YouTube backend configuration

use std::{path::PathBuf, time::Duration};

use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtDlpExtractorConfig {
    pub cookies_path: Option<String>,
}

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
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "staged", alias = "combined", alias = "ffmpegconcat", alias = "concat")]
    Staged,
    #[serde(rename = "direct", alias = "passthrough")]
    Direct,
    #[serde(rename = "relay", alias = "proxy")]
    Relay,
}

impl AudioDeliveryMode {
    pub fn is_direct_like(self) -> bool {
        matches!(self, Self::Auto | Self::Direct)
    }
}

/// Background URL extraction policy for queued tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundExtractMode {
    /// Extract only a bounded near-future window after playback is confirmed.
    #[default]
    Balanced,
    /// Extract URLs for the full remaining queue as soon as startup is armed.
    Performance,
}

/// Audio streaming configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio delivery mode
    #[serde(alias = "source")]
    pub delivery_mode: AudioDeliveryMode,
    /// Cache directory for audio prefixes
    pub cache_dir: Option<PathBuf>,
    /// Prefix size in bytes (default: 200KB)
    pub prefix_size: u64,
    /// Maximum cache size in bytes (default: 200MB)
    pub max_cache_size: u64,
    /// Background extraction policy for queued tracks.
    pub background_extract_mode: BackgroundExtractMode,
    /// Number of future tracks to include in bounded extract/prefix windows.
    pub future_track_count: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            delivery_mode: AudioDeliveryMode::default(),
            cache_dir: None,
            prefix_size: 204_800,
            max_cache_size: 209_715_200,
            background_extract_mode: BackgroundExtractMode::default(),
            future_track_count: 2,
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

    /// Directory to dump raw ytmapi requests/responses for debugging.
    pub request_dump_dir: Option<PathBuf>,

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

    pub yt_dlp_extractor_args: Vec<String>,
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
            request_dump_dir: None,
            cache_duration: Duration::from_secs(3600),
            max_search_results: 50,
            extractor: ExtractorType::default(), // ytx by default (fast, ~200ms)
            yt_dlp_extractor_args: Vec::new(),
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
    use super::{AudioConfig, AudioDeliveryMode, BackgroundExtractMode};

    #[test]
    fn audio_delivery_mode_accepts_current_values() {
        let auto: AudioConfig = toml::from_str("delivery_mode = \"auto\"").expect("parse auto");
        let direct: AudioConfig =
            toml::from_str("delivery_mode = \"direct\"").expect("parse direct");
        let staged: AudioConfig =
            toml::from_str("delivery_mode = \"staged\"").expect("parse staged");
        let relay: AudioConfig = toml::from_str("delivery_mode = \"relay\"").expect("parse relay");

        assert_eq!(auto.delivery_mode, AudioDeliveryMode::Auto);
        assert_eq!(direct.delivery_mode, AudioDeliveryMode::Direct);
        assert_eq!(staged.delivery_mode, AudioDeliveryMode::Staged);
        assert_eq!(relay.delivery_mode, AudioDeliveryMode::Relay);
    }

    #[test]
    fn audio_delivery_mode_accepts_legacy_aliases() {
        let passthrough: AudioConfig =
            toml::from_str("delivery_mode = \"passthrough\"").expect("parse passthrough alias");
        let ffmpegconcat: AudioConfig =
            toml::from_str("delivery_mode = \"ffmpegconcat\"").expect("parse ffmpegconcat alias");
        let concat: AudioConfig =
            toml::from_str("delivery_mode = \"concat\"").expect("parse concat alias");
        let combined_legacy: AudioConfig =
            toml::from_str("source = \"combined\"").expect("parse legacy source key");

        assert_eq!(passthrough.delivery_mode, AudioDeliveryMode::Direct);
        assert_eq!(ffmpegconcat.delivery_mode, AudioDeliveryMode::Staged);
        assert_eq!(concat.delivery_mode, AudioDeliveryMode::Staged);
        assert_eq!(combined_legacy.delivery_mode, AudioDeliveryMode::Staged);
    }

    #[test]
    fn audio_delivery_mode_default_is_auto() {
        assert_eq!(AudioDeliveryMode::default(), AudioDeliveryMode::Auto);
    }

    #[test]
    fn background_extract_mode_defaults_to_balanced() {
        assert_eq!(AudioConfig::default().background_extract_mode, BackgroundExtractMode::Balanced);
    }

    #[test]
    fn background_extract_mode_accepts_current_values() {
        let balanced: AudioConfig =
            toml::from_str("background_extract_mode = \"balanced\"").expect("parse balanced");
        let performance: AudioConfig =
            toml::from_str("background_extract_mode = \"performance\"").expect("parse performance");

        assert_eq!(balanced.background_extract_mode, BackgroundExtractMode::Balanced);
        assert_eq!(performance.background_extract_mode, BackgroundExtractMode::Performance);
    }

    #[test]
    fn future_track_count_defaults_to_two() {
        assert_eq!(AudioConfig::default().future_track_count, 2);
    }

    #[test]
    fn future_track_count_accepts_configured_value() {
        let configured: AudioConfig =
            toml::from_str("future_track_count = 4").expect("parse future_track_count");

        assert_eq!(configured.future_track_count, 4);
    }
}
