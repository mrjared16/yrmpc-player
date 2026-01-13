use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use super::cache::{AudioCache, CacheConfig};
use super::mpv_source::MpvInput;
use super::sources::concat::FfmpegConcatSource;

// ============================================================================
// MpvInput Tests
// ============================================================================

#[test]
fn test_mpv_input_new() {
    let input = MpvInput::new("http://example.com/audio.m4a");
    assert_eq!(input.url, "http://example.com/audio.m4a");
    assert!(input.mpv_args.is_empty());
}

#[test]
fn test_mpv_input_with_args() {
    let args = vec!["--arg1".to_string(), "--arg2".to_string()];
    let input = MpvInput::with_args("http://example.com/audio.m4a", args.clone());
    assert_eq!(input.url, "http://example.com/audio.m4a");
    assert_eq!(input.mpv_args, args);
}

#[test]
fn test_mpv_input_into_string() {
    let input = MpvInput::new("test_url");
    assert_eq!(input.url, "test_url");
}

// ============================================================================
// AudioCache Tests
// ============================================================================

#[test]
fn test_audio_cache_new() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    
    assert!(!cache.has_prefix("test_video_id"));
    assert_eq!(cache.total_size(), 0);
}

#[test]
fn test_audio_cache_with_defaults() {
    let cache = AudioCache::with_defaults().unwrap();
    assert!(!cache.has_prefix("test"));
    assert_eq!(cache.total_size(), 0);
}

#[test]
fn test_audio_cache_path() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    let path = cache.cache_path("dQw4w9WgXcQ");
    assert_eq!(path, temp_dir.path().join("dQw4w9WgXcQ.m4a"));
}

#[test]
fn test_audio_cache_register_and_get() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    
    let path = temp_dir.path().join("test.m4a");
    cache.register_prefix("video1", path.clone(), 500, 10000);
    
    assert!(cache.has_prefix("video1"));
    assert_eq!(cache.get_content_length("video1"), Some(10000));
    assert_eq!(cache.total_size(), 500);
}

#[test]
fn test_audio_cache_multiple_entries() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    
    let path1 = temp_dir.path().join("video1.m4a");
    let path2 = temp_dir.path().join("video2.m4a");
    
    cache.register_prefix("video1", path1, 200, 5000);
    cache.register_prefix("video2", path2, 300, 6000);
    
    assert!(cache.has_prefix("video1"));
    assert!(cache.has_prefix("video2"));
    assert_eq!(cache.get_content_length("video1"), Some(5000));
    assert_eq!(cache.get_content_length("video2"), Some(6000));
    assert_eq!(cache.total_size(), 500);
}

#[test]
fn test_audio_cache_get_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    
    assert!(!cache.has_prefix("nonexistent"));
    assert_eq!(cache.get_content_length("nonexistent"), None);
}

#[test]
fn test_audio_cache_lru_eviction() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 100,
        max_cache_size: 250,
    };
    let cache = AudioCache::new(config).unwrap();
    
    // Create actual files so eviction can delete them
    let path1 = temp_dir.path().join("video1.m4a");
    let path2 = temp_dir.path().join("video2.m4a");
    let path3 = temp_dir.path().join("video3.m4a");
    
    std::fs::write(&path1, vec![0u8; 100]).unwrap();
    std::fs::write(&path2, vec![0u8; 100]).unwrap();
    std::fs::write(&path3, vec![0u8; 100]).unwrap();
    
    cache.register_prefix("video1", path1.clone(), 100, 1000);
    std::thread::sleep(std::time::Duration::from_millis(10));
    cache.register_prefix("video2", path2.clone(), 100, 1000);
    std::thread::sleep(std::time::Duration::from_millis(10));
    cache.register_prefix("video3", path3.clone(), 100, 1000);
    
    // Total is 300, max is 250, so eviction should remove oldest
    cache.evict_lru().unwrap();
    
    // video1 should be evicted (oldest)
    assert!(!cache.has_prefix("video1"));
    assert!(cache.has_prefix("video2"));
    assert!(cache.has_prefix("video3"));
    assert_eq!(cache.total_size(), 200);
}

#[test]
fn test_audio_cache_touch_updates_lru() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 100,
        max_cache_size: 250,
    };
    let cache = AudioCache::new(config).unwrap();
    
    let path1 = temp_dir.path().join("video1.m4a");
    let path2 = temp_dir.path().join("video2.m4a");
    let path3 = temp_dir.path().join("video3.m4a");
    
    std::fs::write(&path1, vec![0u8; 100]).unwrap();
    std::fs::write(&path2, vec![0u8; 100]).unwrap();
    std::fs::write(&path3, vec![0u8; 100]).unwrap();
    
    cache.register_prefix("video1", path1.clone(), 100, 1000);
    std::thread::sleep(std::time::Duration::from_millis(10));
    cache.register_prefix("video2", path2.clone(), 100, 1000);
    std::thread::sleep(std::time::Duration::from_millis(10));
    
    // Touch video1 to make it more recent than video2
    cache.touch("video1");
    std::thread::sleep(std::time::Duration::from_millis(10));
    
    cache.register_prefix("video3", path3.clone(), 100, 1000);
    
    // Now evict - video2 should be oldest
    cache.evict_lru().unwrap();
    
    assert!(cache.has_prefix("video1"));
    assert!(!cache.has_prefix("video2")); // video2 evicted, not video1
    assert!(cache.has_prefix("video3"));
}

#[test]
fn test_audio_cache_prefix_size() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 2048,
        max_cache_size: 10240,
    };
    let cache = AudioCache::new(config).unwrap();
    assert_eq!(cache.prefix_size(), 2048);
}

#[test]
fn test_audio_cache_eviction_multiple_rounds() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 100,
        max_cache_size: 250, // Allow 2 entries (200 bytes) to remain
    };
    let cache = AudioCache::new(config).unwrap();
    
    // Add 4 entries (400 bytes total)
    for i in 1..=4 {
        let path = temp_dir.path().join(format!("video{}.m4a", i));
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        cache.register_prefix(&format!("video{}", i), path, 100, 1000);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    
    // Evict until under limit (150) - should remove video1 and video2
    cache.evict_lru().unwrap();
    
    assert!(!cache.has_prefix("video1"));
    assert!(!cache.has_prefix("video2"));
    assert!(cache.has_prefix("video3"));
    assert!(cache.has_prefix("video4"));
    assert!(cache.total_size() <= 250);
}

// ============================================================================
// FfmpegConcatSource Tests
// ============================================================================

#[test]
fn test_ffmpeg_concat_source_build_url() {
    let url = FfmpegConcatSource::build_concat_url(
        &PathBuf::from("/cache/video.m4a"),
        200000,
        "https://youtube.com/stream",
    );
    assert_eq!(
        url,
        "concat:/cache/video.m4a|subfile,,start,200000,end,0,,:https://youtube.com/stream"
    );
}

#[test]
fn test_ffmpeg_concat_source_build_url_with_spaces() {
    let url = FfmpegConcatSource::build_concat_url(
        &PathBuf::from("/cache/my video.m4a"),
        204800,
        "https://example.com/stream?id=123",
    );
    assert_eq!(
        url,
        "concat:/cache/my video.m4a|subfile,,start,204800,end,0,,:https://example.com/stream?id=123"
    );
}

#[test]
fn test_ffmpeg_concat_source_protocol_whitelist() {
    let args = FfmpegConcatSource::protocol_whitelist_args();
    assert_eq!(args.len(), 1);
    assert!(args[0].contains("protocol_whitelist"));
    assert!(args[0].contains("concat"));
    assert!(args[0].contains("subfile"));
    assert!(args[0].contains("file"));
    assert!(args[0].contains("http"));
    assert!(args[0].contains("https"));
}

#[test]
fn test_ffmpeg_concat_source_creation() {
    let temp_dir = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp_dir.path().to_path_buf(),
        prefix_size: 1024,
        max_cache_size: 10240,
    };
    let cache = Arc::new(AudioCache::new(config).unwrap());
    
    let url_resolver = Box::new(|_video_id: &str| -> anyhow::Result<String> {
        Ok("https://test.com/stream".to_string())
    });
    
    let _source = FfmpegConcatSource::new(cache, url_resolver);
}

// ============================================================================
// CacheConfig Tests
// ============================================================================

#[test]
fn test_cache_config_default() {
    let config = CacheConfig::default();
    assert!(config.cache_dir.to_string_lossy().contains("rmpc"));
    assert!(config.cache_dir.to_string_lossy().contains("audio"));
    assert_eq!(config.prefix_size, 204_800); // 200KB
    assert_eq!(config.max_cache_size, 209_715_200); // 200MB
}

#[test]
fn test_cache_config_custom() {
    let custom_path = PathBuf::from("/tmp/custom_cache");
    let config = CacheConfig {
        cache_dir: custom_path.clone(),
        prefix_size: 512_000,
        max_cache_size: 104_857_600,
    };
    
    assert_eq!(config.cache_dir, custom_path);
    assert_eq!(config.prefix_size, 512_000);
    assert_eq!(config.max_cache_size, 104_857_600);
}
