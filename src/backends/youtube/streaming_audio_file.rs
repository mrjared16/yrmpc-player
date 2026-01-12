use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use crate::backends::youtube::range_set::RangeSet;

/// Typed download errors for match-based retry logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// Network connection failed (retryable)
    Network(String),
    /// HTTP error with status code (4xx = permanent, 5xx = retryable)
    Http { status: u16, message: String },
    /// URL expired, needs refresh (retryable with new URL)
    UrlExpired,
    /// File I/O error (usually permanent)
    Io(String),
    /// Download was cancelled
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Http { status, message } => write!(f, "HTTP {status}: {message}"),
            Self::UrlExpired => write!(f, "URL expired"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Cancelled => write!(f, "Download cancelled"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl DownloadError {
    /// Check if this error is retryable
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Http { status, .. } => *status >= 500,
            Self::UrlExpired => true,
            Self::Io(_) => false,
            Self::Cancelled => false,
        }
    }

    /// Check if URL needs refresh before retry
    #[must_use]
    pub fn needs_url_refresh(&self) -> bool {
        matches!(self, Self::UrlExpired | Self::Http { status: 403, .. })
    }
}

/// A streaming audio file that supports concurrent reading and writing.
///
/// Design inspired by librespot's `AudioFile`:
/// - Pre-allocates file to `content_length`
/// - Background writer appends downloaded chunks
/// - Reader blocks if requested bytes not yet available
/// - Condvar signals when new bytes arrive
#[derive(Debug)]
pub struct ProgressiveAudioFile {
    inner: Arc<Mutex<ProgressiveAudioFileInner>>,
    condvar: Arc<Condvar>,
}

#[derive(Debug)]
struct ProgressiveAudioFileInner {
    path: PathBuf,
    content_length: u64,
    downloaded: RangeSet,
    write_file: Option<File>,
    error: Option<DownloadError>,
    requested_offset: Option<u64>,
}

impl ProgressiveAudioFile {
    /// Create a new streaming audio file.
    /// Pre-allocates the file to `content_length`.
    ///
    /// # Errors
    ///
    /// Returns an error if file creation or allocation fails.
    pub fn new(path: impl AsRef<Path>, content_length: u64) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).write(true).truncate(true).open(&path)?;

        file.set_len(content_length)?;

        Ok(Self {
            inner: Arc::new(Mutex::new(ProgressiveAudioFileInner {
                path,
                content_length,
                downloaded: RangeSet::new(),
                write_file: Some(file),
                error: None,
                requested_offset: None,
            })),
            condvar: Arc::new(Condvar::new()),
        })
    }

    /// Write data at the specified offset.
    /// Called by background download task.
    /// Notifies waiting readers via condvar.
    ///
    /// # Errors
    ///
    /// Returns an error if file I/O operations fail.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned (another thread panicked while holding
    /// the lock).
    pub fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();

        if let Some(ref file) = inner.write_file {
            file.write_at(data, offset)?;

            inner.downloaded.add_range(offset, offset + data.len() as u64);

            if inner.downloaded.is_complete(inner.content_length) {
                inner.write_file = None;
            }
        }

        self.condvar.notify_all();

        Ok(())
    }

    /// Read data at offset into buffer.
    /// BLOCKS if bytes not yet available.
    /// Returns number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns an error if file I/O operations fail or download has failed.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let (available, path) = {
            let mut inner = self.inner.lock().unwrap();

            loop {
                if let Some(ref err) = inner.error {
                    return Err(io::Error::other(err.to_string()));
                }

                let available = inner.downloaded.contiguous_from(offset);
                if available > offset {
                    break (available - offset, inner.path.clone());
                }

                let wait_result = self.condvar.wait_timeout(inner, Duration::from_secs(30)).unwrap();
                inner = wait_result.0;
                if wait_result.1.timed_out() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "Read timed out waiting for data"));
                }
            }
        };

        let bytes_to_read = std::cmp::min(buf.len() as u64, available) as usize;

        let read_file = File::open(&path)?;
        read_file.read_at(&mut buf[..bytes_to_read], offset)?;

        Ok(bytes_to_read)
    }

    #[must_use]
    pub fn bytes_available_from(&self, offset: u64) -> u64 {
        let inner = self.inner.lock().unwrap();
        let end = inner.downloaded.contiguous_from(offset);
        if end > offset { end - offset } else { 0 }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.downloaded.is_complete(inner.content_length)
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        let inner = self.inner.lock().unwrap();
        inner.path.clone()
    }

    pub fn set_error(&self, error: DownloadError) {
        let mut inner = self.inner.lock().unwrap();
        inner.error = Some(error);
        self.condvar.notify_all();
    }

    #[must_use]
    pub fn get_error(&self) -> Option<DownloadError> {
        let inner = self.inner.lock().unwrap();
        inner.error.clone()
    }

    #[must_use]
    pub fn has_error(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.error.is_some()
    }

    #[must_use]
    pub fn content_length(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner.content_length
    }

    #[must_use]
    pub fn downloaded_bytes(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner.downloaded.total_bytes()
    }

    #[must_use]
    pub fn first_gap_after(&self, offset: u64) -> Option<(u64, u64)> {
        let inner = self.inner.lock().unwrap();
        if inner.downloaded.is_complete(inner.content_length) {
            return None;
        }
        inner.downloaded.first_gap_after(offset, inner.content_length)
    }

    /// Request priority download starting from offset.
    /// Called when user seeks to undownloaded region.
    pub fn request_range(&self, start_offset: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.requested_offset = Some(start_offset);
        // Signal any waiting download task
        self.condvar.notify_all();
    }

    /// Get the requested offset if any (for download task to check).
    #[must_use]
    pub fn get_requested_offset(&self) -> Option<u64> {
        self.inner.lock().unwrap().requested_offset
    }

    /// Clear the requested offset after download starts from there.
    pub fn clear_requested_offset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.requested_offset = None;
    }
}

impl Clone for ProgressiveAudioFile {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner), condvar: Arc::clone(&self.condvar) }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_create_and_preallocate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 1000).unwrap();

        assert_eq!(file.content_length(), 1000);
        assert_eq!(file.downloaded_bytes(), 0);
        assert!(!file.is_complete());
        assert!(path.exists());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1000);
    }

    #[test]
    fn test_write_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 100).unwrap();

        file.write_at(0, b"hello").unwrap();

        let mut buf = [0u8; 5];
        let n = file.read_at(0, &mut buf).unwrap();

        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn test_bytes_available() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 100).unwrap();

        assert_eq!(file.bytes_available_from(0), 0);

        file.write_at(0, &[0u8; 50]).unwrap();
        assert_eq!(file.bytes_available_from(0), 50);
        assert_eq!(file.bytes_available_from(25), 25);
        assert_eq!(file.bytes_available_from(50), 0);
    }

    #[test]
    fn test_complete_detection() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 100).unwrap();

        file.write_at(0, &[0u8; 100]).unwrap();
        assert!(file.is_complete());
    }

    #[test]
    fn test_concurrent_write_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 100).unwrap();
        let file_clone = file.clone();

        let writer = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(50));
            file_clone.write_at(0, &[42u8; 100]).unwrap();
        });

        let mut buf = [0u8; 10];
        let n = file.read_at(0, &mut buf).unwrap();

        writer.join().unwrap();

        assert_eq!(n, 10);
        assert_eq!(buf, [42u8; 10]);
    }

    #[test]
    fn test_error_propagation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.webm");

        let file = ProgressiveAudioFile::new(&path, 100).unwrap();
        file.set_error(DownloadError::Network("Download failed".to_string()));

        assert!(file.has_error());
        assert!(file.get_error().is_some());

        let mut buf = [0u8; 10];
        let result = file.read_at(0, &mut buf);
        assert!(result.is_err());
    }
}
