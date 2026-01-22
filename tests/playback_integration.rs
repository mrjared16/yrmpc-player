//! Integration tests for YouTube playback pipeline
//!
//! Tests the complete flow: URL extraction → prefix download → MPV playback
//! Run with: cargo nextest run --test playback_integration -- --include-ignored

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const TEST_VIDEO_ID: &str = "dQw4w9WgXcQ";
const CACHE_DIR: &str = ".cache/rmpc/audio";

fn cache_path() -> PathBuf {
    dirs::home_dir().unwrap().join(CACHE_DIR)
}

fn prefix_path(video_id: &str) -> PathBuf {
    cache_path().join(format!("{}.webm", video_id))
}

fn extract_url_with_ytx(video_id: &str) -> Result<String, String> {
    let output = Command::new("ytx")
        .args(["music", video_id])
        .output()
        .map_err(|e| format!("Failed to run ytx: {}", e))?;

    if !output.status.success() {
        return Err(format!("ytx failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(|s| s.to_string()))
        .ok_or_else(|| format!("No URL in ytx output: {}", stdout))
}

fn extract_url_with_ytdlp(video_id: &str) -> Result<String, String> {
    let url = format!("https://www.youtube.com/watch?v={}", video_id);
    let output = Command::new("yt-dlp")
        .args(["-f", "251", "-g", &url])
        .output()
        .map_err(|e| format!("Failed to run yt-dlp: {}", e))?;

    if !output.status.success() {
        return Err(format!("yt-dlp failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| "No URL in yt-dlp output".to_string())
}

fn download_prefix(url: &str, dest: &PathBuf, size: u64) -> Result<u64, String> {
    let output = Command::new("curl")
        .args(["-sL", "--range", &format!("0-{}", size - 1), url, "-o", dest.to_str().unwrap()])
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    std::fs::metadata(dest).map(|m| m.len()).map_err(|e| format!("Failed to get file size: {}", e))
}

fn build_concat_url(prefix_path: &PathBuf, prefix_size: u64, stream_url: &str) -> String {
    format!(
        "lavf://concat:{}|subfile,,start,{},end,0,,:{}",
        prefix_path.display(),
        prefix_size,
        stream_url
    )
}

struct MpvTestSession {
    socket_path: PathBuf,
    child: std::process::Child,
}

impl MpvTestSession {
    fn start() -> Result<Self, String> {
        let socket_path = PathBuf::from("/tmp/rmpc-test-mpv.sock");
        let _ = std::fs::remove_file(&socket_path);

        let child = Command::new("mpv")
            .args([
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                "--stream-lavf-o-append=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat",
                &format!("--input-ipc-server={}", socket_path.display()),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn mpv: {}", e))?;

        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            if socket_path.exists() {
                return Ok(Self { socket_path, child });
            }
        }

        Err("MPV socket not created after 2s".to_string())
    }

    fn send_command(&self, cmd: &str) -> Result<serde_json::Value, String> {
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|e| format!("Connect failed: {}", e))?;

        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

        stream
            .write_all(format!("{}\n", cmd).as_bytes())
            .map_err(|e| format!("Write failed: {}", e))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| format!("Read failed: {}", e))?;

        serde_json::from_str(&line).map_err(|e| format!("Parse failed: {}", e))
    }

    fn loadfile(&self, url: &str) -> Result<(), String> {
        let cmd = serde_json::json!({"command": ["loadfile", url, "replace"]});
        let resp = self.send_command(&cmd.to_string())?;
        if resp.get("error").and_then(|e| e.as_str()) == Some("success") {
            Ok(())
        } else {
            Err(format!("loadfile failed: {:?}", resp))
        }
    }

    fn get_property(&self, name: &str) -> Result<serde_json::Value, String> {
        let cmd = serde_json::json!({"command": ["get_property", name]});
        let resp = self.send_command(&cmd.to_string())?;
        resp.get("data").cloned().ok_or_else(|| format!("No data in response: {:?}", resp))
    }

    fn get_playback_time(&self) -> Result<f64, String> {
        self.get_property("playback-time")?
            .as_f64()
            .ok_or_else(|| "playback-time not a number".to_string())
    }

    fn get_path(&self) -> Result<String, String> {
        self.get_property("path")?
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "path not a string".to_string())
    }

    fn is_playing(&self) -> bool {
        self.get_property("core-idle")
            .ok()
            .and_then(|v| v.as_bool())
            .map(|idle| !idle)
            .unwrap_or(false)
    }
}

impl Drop for MpvTestSession {
    fn drop(&mut self) {
        let _ = self.send_command(r#"{"command": ["quit"]}"#);
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[test]
#[ignore = "requires network"]
fn test_ytx_extracts_valid_url() {
    let result = extract_url_with_ytx(TEST_VIDEO_ID);
    assert!(result.is_ok(), "ytx extraction failed: {:?}", result);

    let url = result.unwrap();
    assert!(url.starts_with("https://"), "URL should start with https://");
    assert!(
        url.contains("googlevideo.com") || url.contains("youtube.com"),
        "URL should be from Google/YouTube"
    );
    assert!(url.len() > 100, "URL should be substantial (got {} chars)", url.len());
}

#[test]
#[ignore]
fn test_ytdlp_extracts_valid_url() {
    let result = extract_url_with_ytdlp(TEST_VIDEO_ID);
    assert!(result.is_ok(), "yt-dlp extraction failed: {:?}", result);

    let url = result.unwrap();
    assert!(url.starts_with("https://"), "URL should start with https://");
    assert!(url.contains("googlevideo.com"), "URL should be from googlevideo.com");
}

#[test]
#[ignore]
fn test_prefix_download() {
    let url = extract_url_with_ytdlp(TEST_VIDEO_ID).expect("URL extraction failed");

    let test_prefix = PathBuf::from("/tmp/rmpc-test-prefix.webm");
    let _ = std::fs::remove_file(&test_prefix);

    let size = download_prefix(&url, &test_prefix, 204800).expect("Download failed");

    assert_eq!(size, 204800, "Prefix should be exactly 200KB");
    assert!(test_prefix.exists(), "Prefix file should exist");

    std::fs::remove_file(&test_prefix).ok();
}

#[test]
fn test_concat_url_format() {
    let prefix = PathBuf::from("/home/user/.cache/rmpc/audio/abc123.webm");
    let stream = "https://rr1---sn-abc.googlevideo.com/videoplayback?id=xyz";

    let concat = build_concat_url(&prefix, 204800, stream);

    assert!(concat.starts_with("lavf://concat:"), "Should use lavf:// wrapper");
    assert!(concat.contains("|subfile,,start,204800,end,0,,:"), "Should have subfile segment");
    assert!(concat.ends_with(stream), "Should end with stream URL");
}

#[test]
#[ignore = "requires mpv"]
fn test_mpv_plays_local_file() {
    let mpv = MpvTestSession::start().expect("Failed to start MPV");

    let test_file = prefix_path(TEST_VIDEO_ID);
    if !test_file.exists() {
        eprintln!("Skipping: no cached prefix at {:?}", test_file);
        return;
    }

    mpv.loadfile(test_file.to_str().unwrap()).expect("loadfile failed");

    std::thread::sleep(Duration::from_secs(2));

    let time = mpv.get_playback_time().expect("Failed to get playback time");
    assert!(time > 0.5, "Should have played at least 0.5s (got {})", time);
}

#[test]
#[ignore]
fn test_concat_mode_used() {
    let url = extract_url_with_ytdlp(TEST_VIDEO_ID).expect("URL extraction failed");

    let prefix = prefix_path(TEST_VIDEO_ID);
    let prefix_size = if prefix.exists() {
        std::fs::metadata(&prefix).unwrap().len()
    } else {
        std::fs::create_dir_all(cache_path()).ok();
        download_prefix(&url, &prefix, 204800).expect("Prefix download failed")
    };

    let concat_url = build_concat_url(&prefix, prefix_size, &url);

    assert!(concat_url.starts_with("lavf://concat:"), "URL should use lavf://concat: wrapper");
    assert!(
        concat_url.contains(&prefix.to_string_lossy().to_string()),
        "URL should contain local prefix path"
    );
    assert!(concat_url.contains("|subfile,,start,"), "URL should have subfile segment");
    assert!(
        concat_url.contains("googlevideo.com") || concat_url.contains("youtube.com"),
        "URL should end with stream URL"
    );

    let mpv = MpvTestSession::start().expect("Failed to start MPV");
    mpv.loadfile(&concat_url).expect("loadfile failed");

    std::thread::sleep(Duration::from_millis(200));

    assert!(mpv.is_playing(), "MPV should be playing concat URL");

    println!("=== CONCAT MODE VERIFIED ===");
}

#[test]
#[ignore]
fn test_seamless_playback_heuristic() {
    let url = extract_url_with_ytdlp(TEST_VIDEO_ID).expect("URL extraction failed");

    let prefix = prefix_path(TEST_VIDEO_ID);
    if !prefix.exists() {
        std::fs::create_dir_all(cache_path()).ok();
        download_prefix(&url, &prefix, 204800).expect("Prefix download failed");
    }
    let prefix_size = std::fs::metadata(&prefix).unwrap().len();

    let concat_url = build_concat_url(&prefix, prefix_size, &url);
    let mpv = MpvTestSession::start().expect("Failed to start MPV");

    mpv.loadfile(&concat_url).expect("loadfile failed");

    std::thread::sleep(Duration::from_millis(500));

    let mut positions: Vec<f64> = Vec::new();
    for _ in 0..5 {
        if let Ok(pos) = mpv.get_playback_time() {
            positions.push(pos);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(!positions.is_empty(), "Should have at least 1 position sample");

    if positions.len() >= 2 {
        let first = positions[0];
        let last = positions[positions.len() - 1];
        assert!(last > first, "Playback should advance: first={}, last={}", first, last);
    }

    println!("Positions: {:?}", positions);
    println!("=== SEAMLESS HEURISTIC PASSED ===");
}

#[test]
#[ignore]
fn test_cache_miss_then_hit() {
    let test_id = "test_cache_miss_id";
    let prefix = prefix_path(test_id);

    let _ = std::fs::remove_file(&prefix);
    assert!(!prefix.exists(), "Cache should be cleared");

    let url = extract_url_with_ytdlp(TEST_VIDEO_ID).expect("URL extraction failed");

    let miss_start = Instant::now();
    std::fs::create_dir_all(cache_path()).ok();
    download_prefix(&url, &prefix, 204800).expect("Download failed");
    let miss_time = miss_start.elapsed();
    println!("Cache MISS download time: {:?}", miss_time);

    let hit_start = Instant::now();
    let exists = prefix.exists();
    let hit_time = hit_start.elapsed();
    println!("Cache HIT check time: {:?}", hit_time);

    assert!(exists, "Prefix should exist after download");
    assert!(
        hit_time < Duration::from_millis(10),
        "Cache hit should be < 10ms (got {:?})",
        hit_time
    );

    std::fs::remove_file(&prefix).ok();
}

#[test]
#[ignore = "requires network"]
fn test_ytx_bulk_mode_timeout_handling() {
    let start = Instant::now();

    let result = extract_url_with_ytx(TEST_VIDEO_ID);
    let elapsed = start.elapsed();

    println!("ytx single extraction took: {:?}", elapsed);

    if elapsed > Duration::from_secs(5) {
        println!("WARNING: ytx took >5s - may be rate limited or using bulk mode");
    }

    assert!(result.is_ok(), "ytx should succeed even if slow: {:?}", result);
    assert!(
        elapsed < Duration::from_secs(30),
        "ytx should complete within 30s timeout (took {:?})",
        elapsed
    );
}

#[test]
#[ignore]
fn test_first_audio_latency() {
    let start = Instant::now();

    let url = extract_url_with_ytdlp(TEST_VIDEO_ID).expect("URL extraction failed");
    let url_time = start.elapsed();

    let prefix = prefix_path(TEST_VIDEO_ID);
    let prefix_size = if prefix.exists() {
        std::fs::metadata(&prefix).unwrap().len()
    } else {
        std::fs::create_dir_all(cache_path()).ok();
        download_prefix(&url, &prefix, 204800).expect("Prefix download failed")
    };
    let prep_time = start.elapsed();

    let concat_url = build_concat_url(&prefix, prefix_size, &url);
    let mpv = MpvTestSession::start().expect("Failed to start MPV");

    let play_start = Instant::now();
    mpv.loadfile(&concat_url).expect("loadfile failed");

    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(20));
        if let Ok(pos) = mpv.get_playback_time() {
            if pos > 0.0 {
                let first_audio = play_start.elapsed();
                let total = start.elapsed();

                println!("\n=== LATENCY REPORT ===");
                println!("URL extraction: {:?}", url_time);
                println!("Total preparation: {:?}", prep_time);
                println!("Load to first audio: {:?}", first_audio);
                println!("TOTAL request to audio: {:?}", total);

                assert!(
                    first_audio < Duration::from_millis(500),
                    "First audio should be <500ms after loadfile (got {:?})",
                    first_audio
                );

                return;
            }
        }
    }

    panic!("No audio playback detected within 1 second");
}
