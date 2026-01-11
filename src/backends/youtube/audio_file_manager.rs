use std::{collections::HashMap, io, path::PathBuf, sync::Arc};

use parking_lot::RwLock;

use crate::backends::youtube::streaming_audio_file::StreamingAudioFile;

/// Prefetch priority for a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefetchPriority {
    /// Currently playing - full download
    Current,
    /// Next track - full download  
    Next,
    /// Next+2 or Next+3 - 30 second prefetch only
    Partial { duration_secs: u32 },
    /// Not in prefetch window - can be evicted
    None,
}

/// Configuration for audio file manager.
#[derive(Debug, Clone)]
pub struct AudioFileManagerConfig {
    /// Directory to store streaming audio files.
    pub cache_dir: PathBuf,
    /// Maximum total cache size in bytes (for LRU eviction).
    pub max_size_bytes: u64,
}

impl Default for AudioFileManagerConfig {
    fn default() -> Self {
        Self {
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rmpc")
                .join("streaming"),
            max_size_bytes: 100 * 1024 * 1024, // 100 MB
        }
    }
}

/// Metadata for a managed audio file.
#[derive(Debug)]
struct ManagedFile {
    file: Arc<StreamingAudioFile>,
    video_id: String,
    content_length: u64,
    last_accessed: std::time::Instant,
    priority: PrefetchPriority,
}

/// Manages multiple StreamingAudioFile instances.
///
/// Provides:
/// - Get-or-create semantics by video_id
/// - File path generation based on video_id
/// - Future: LRU eviction when cache exceeds max_size_bytes
pub struct AudioFileManager {
    config: AudioFileManagerConfig,
    files: RwLock<HashMap<String, ManagedFile>>,
}

impl AudioFileManager {
    /// Create a new manager with default configuration.
    pub fn new() -> Self {
        Self::with_config(AudioFileManagerConfig::default())
    }

    /// Create a new manager with custom configuration.
    pub fn with_config(config: AudioFileManagerConfig) -> Self {
        // Ensure cache directory exists
        let _ = std::fs::create_dir_all(&config.cache_dir);

        Self { config, files: RwLock::new(HashMap::new()) }
    }

    /// Get or create a streaming audio file for the given video.
    ///
    /// If file exists, returns existing Arc.
    /// If not, creates new StreamingAudioFile and returns Arc.
    pub fn get_or_create(
        &self,
        video_id: &str,
        content_length: u64,
    ) -> io::Result<Arc<StreamingAudioFile>> {
        // Fast path: check if already exists (read lock)
        {
            let files = self.files.read();
            if let Some(managed) = files.get(video_id) {
                return Ok(Arc::clone(&managed.file));
            }
        }

        // Slow path: create new file (write lock)
        let mut files = self.files.write();

        // Double-check after acquiring write lock
        if let Some(managed) = files.get(video_id) {
            return Ok(Arc::clone(&managed.file));
        }

        // Create new streaming file
        let path = self.file_path(video_id);
        let file = StreamingAudioFile::new(&path, content_length)?;
        let arc_file = Arc::new(file);

        files.insert(video_id.to_string(), ManagedFile {
            file: Arc::clone(&arc_file),
            video_id: video_id.to_string(),
            content_length,
            last_accessed: std::time::Instant::now(),
            priority: PrefetchPriority::None,
        });

        Ok(arc_file)
    }

    /// Get existing file if it exists.
    pub fn get(&self, video_id: &str) -> Option<Arc<StreamingAudioFile>> {
        let files = self.files.read();
        files.get(video_id).map(|m| Arc::clone(&m.file))
    }

    /// Remove a file from management (and optionally delete from disk).
    pub fn remove(&self, video_id: &str, delete_file: bool) {
        let mut files = self.files.write();
        if let Some(managed) = files.remove(video_id) {
            if delete_file {
                let _ = std::fs::remove_file(managed.file.path());
            }
        }
    }

    /// Get the file path for a video_id.
    pub fn file_path(&self, video_id: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{}.webm.part", video_id))
    }

    /// Get total bytes currently managed.
    pub fn total_bytes(&self) -> u64 {
        let files = self.files.read();
        files.values().map(|m| m.content_length).sum()
    }

    /// Get count of managed files.
    pub fn file_count(&self) -> usize {
        self.files.read().len()
    }

    /// Update last accessed time for a file (for LRU).
    pub fn touch(&self, video_id: &str) {
        let mut files = self.files.write();
        if let Some(managed) = files.get_mut(video_id) {
            managed.last_accessed = std::time::Instant::now();
        }
    }

    /// Evict least-recently-used files to stay under max_size_bytes.
    /// Never evicts Current or Next priority files.
    pub fn evict_if_needed(&self) {
        let mut files = self.files.write();

        let current_size: u64 = files.values().map(|m| m.content_length).sum();

        if current_size <= self.config.max_size_bytes {
            return;
        }

        let mut candidates: Vec<_> = files
            .iter()
            .filter(|(_, m)| {
                !matches!(m.priority, PrefetchPriority::Current | PrefetchPriority::Next)
            })
            .map(|(id, m)| (id.clone(), m.last_accessed, m.content_length))
            .collect();

        candidates.sort_by_key(|(_, accessed, _)| *accessed);

        let mut size_to_free = current_size.saturating_sub(self.config.max_size_bytes);
        let mut to_remove = Vec::new();

        for (video_id, _, size) in candidates {
            if size_to_free == 0 {
                break;
            }
            to_remove.push(video_id.clone());
            size_to_free = size_to_free.saturating_sub(size);
        }

        for video_id in to_remove {
            if let Some(managed) = files.remove(&video_id) {
                let _ = std::fs::remove_file(managed.file.path());
            }
        }
    }

    /// Update prefetch priorities based on queue position.
    /// Call this when queue changes or track advances.
    pub fn update_prefetch_window(&self, queue: &[String]) {
        let mut files = self.files.write();

        for managed in files.values_mut() {
            managed.priority = PrefetchPriority::None;
        }

        for (i, video_id) in queue.iter().take(4).enumerate() {
            if let Some(managed) = files.get_mut(video_id) {
                managed.priority = match i {
                    0 => PrefetchPriority::Current,
                    1 => PrefetchPriority::Next,
                    2 | 3 => PrefetchPriority::Partial { duration_secs: 30 },
                    _ => PrefetchPriority::None,
                };
            }
        }
    }

    /// Get prefetch priority for a video.
    pub fn get_priority(&self, video_id: &str) -> PrefetchPriority {
        self.files.read().get(video_id).map(|m| m.priority).unwrap_or(PrefetchPriority::None)
    }

    /// Calculate byte limit for partial prefetch.
    /// 30 seconds at given bitrate.
    pub fn partial_prefetch_bytes(bitrate_bps: u32) -> u64 {
        (30 * bitrate_bps / 8) as u64
    }
}

impl Default for AudioFileManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn test_config(dir: &std::path::Path) -> AudioFileManagerConfig {
        AudioFileManagerConfig { cache_dir: dir.to_path_buf(), max_size_bytes: 100 * 1024 * 1024 }
    }

    #[test]
    fn test_get_or_create() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        let file1 = manager.get_or_create("video1", 1000).unwrap();
        let file2 = manager.get_or_create("video1", 1000).unwrap();

        assert!(Arc::ptr_eq(&file1, &file2));
        assert_eq!(manager.file_count(), 1);
    }

    #[test]
    fn test_multiple_videos() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        let _file1 = manager.get_or_create("video1", 1000).unwrap();
        let _file2 = manager.get_or_create("video2", 2000).unwrap();

        assert_eq!(manager.file_count(), 2);
        assert_eq!(manager.total_bytes(), 3000);
    }

    #[test]
    fn test_get_nonexistent() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        assert!(manager.get("nonexistent").is_none());
    }

    #[test]
    fn test_remove() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        let file = manager.get_or_create("video1", 1000).unwrap();
        let path = file.path();
        assert!(path.exists());

        manager.remove("video1", true);
        assert_eq!(manager.file_count(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn test_file_path() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        let path = manager.file_path("abc123");
        assert!(path.to_string_lossy().contains("abc123.webm.part"));
    }

    #[test]
    fn test_prefetch_window() {
        let dir = tempdir().unwrap();
        let manager = AudioFileManager::with_config(test_config(dir.path()));

        manager.get_or_create("video0", 1000).unwrap();
        manager.get_or_create("video1", 1000).unwrap();
        manager.get_or_create("video2", 1000).unwrap();
        manager.get_or_create("video3", 1000).unwrap();
        manager.get_or_create("video4", 1000).unwrap();

        let queue = vec![
            "video0".to_string(),
            "video1".to_string(),
            "video2".to_string(),
            "video3".to_string(),
            "video4".to_string(),
        ];
        manager.update_prefetch_window(&queue);

        assert_eq!(manager.get_priority("video0"), PrefetchPriority::Current);
        assert_eq!(manager.get_priority("video1"), PrefetchPriority::Next);
        assert_eq!(manager.get_priority("video2"), PrefetchPriority::Partial { duration_secs: 30 });
        assert_eq!(manager.get_priority("video3"), PrefetchPriority::Partial { duration_secs: 30 });
        assert_eq!(manager.get_priority("video4"), PrefetchPriority::None);
    }

    #[test]
    fn test_partial_prefetch_bytes() {
        assert_eq!(AudioFileManager::partial_prefetch_bytes(128000), 480000);

        assert_eq!(AudioFileManager::partial_prefetch_bytes(256000), 960000);
    }

    #[test]
    fn test_lru_eviction() {
        let dir = tempdir().unwrap();
        let config =
            AudioFileManagerConfig { cache_dir: dir.path().to_path_buf(), max_size_bytes: 2500 };
        let manager = AudioFileManager::with_config(config);

        manager.get_or_create("video1", 1000).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        manager.get_or_create("video2", 1000).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        manager.get_or_create("video3", 1000).unwrap();

        manager.evict_if_needed();

        assert!(manager.get("video1").is_none());
        assert!(manager.get("video2").is_some());
        assert!(manager.get("video3").is_some());
    }

    #[test]
    fn test_no_evict_current_next() {
        let dir = tempdir().unwrap();
        let config =
            AudioFileManagerConfig { cache_dir: dir.path().to_path_buf(), max_size_bytes: 1500 };
        let manager = AudioFileManager::with_config(config);

        manager.get_or_create("current", 1000).unwrap();
        manager.get_or_create("next", 1000).unwrap();

        let queue = vec!["current".to_string(), "next".to_string()];
        manager.update_prefetch_window(&queue);

        manager.get_or_create("other", 1000).unwrap();

        manager.evict_if_needed();

        assert!(manager.get("current").is_some());
        assert!(manager.get("next").is_some());
        assert!(manager.get("other").is_none());
    }
}
