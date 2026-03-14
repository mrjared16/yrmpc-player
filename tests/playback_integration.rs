//! Integration tests for YouTube playback pipeline
//!
//! Tests the complete flow: URL extraction → prefix download → MPV playback
//! Run with: cargo nextest run --test playback_integration -- --include-ignored

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        Once,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use crossbeam::channel::{self, Receiver};
use rmpc::backends::youtube::{
    audio::{AudioSourcePlanner, sources::concat::FfmpegConcatSource},
    config::AudioDeliveryMode,
    media::{MediaPreparer, PreparedMedia, PreloadTier},
    server::orchestrator,
    services::{InternalEvent, PlaybackService, PlaybackState, PlaybackStateTracker, QueueService},
    url_resolver::UrlResolver,
};
use rmpc::domain::Song;
use tempfile::TempDir;

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

fn fixture_mp3_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ytmapi-yrmpc/test_json/test_upload.mp3")
}

fn fixture_mp3_bytes() -> Vec<u8> {
    fs::read(fixture_mp3_path()).expect("fixture mp3 should be readable")
}

fn build_playback_service(
    mode: AudioDeliveryMode,
) -> Result<(TempDir, PlaybackService), String> {
    let temp_dir = TempDir::new().map_err(|e| format!("tempdir failed: {e}"))?;
    let socket_path = temp_dir.path().join("mpv.sock");
    let service = PlaybackService::new(
        &socket_path,
        Arc::new(UrlResolver::default()),
        None,
        AudioSourcePlanner.plan(mode),
        None,
    )
    .map_err(|e| format!("playback service failed: {e}"))?;

    Ok((temp_dir, service))
}

fn wait_for_first_audio(
    playback: &PlaybackService,
    timeout: Duration,
) -> Result<Duration, String> {
    let start = Instant::now();
    loop {
        if let Ok(position) = playback.get_position() {
            if position > 0.0 {
                return Ok(start.elapsed());
            }
        }

        if start.elapsed() >= timeout {
            return Err(format!("timed out waiting for playback start after {timeout:?}"));
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_position_at_least(
    playback: &PlaybackService,
    min_position: f64,
    timeout: Duration,
) -> Result<f64, String> {
    let start = Instant::now();
    loop {
        if let Ok(position) = playback.get_position() {
            if position >= min_position {
                return Ok(position);
            }
        }

        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out waiting for playback position >= {min_position:.2}s"
            ));
        }

        thread::sleep(Duration::from_millis(50));
    }
}

struct StaticAudioServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct FixtureServerOptions {
    truncate_after_absolute: Option<usize>,
}

impl StaticAudioServer {
    fn start(bytes: Vec<u8>) -> Result<Self, String> {
        Self::start_with_options(bytes, FixtureServerOptions::default())
    }

    fn start_with_truncation(bytes: Vec<u8>, truncate_after_absolute: usize) -> Result<Self, String> {
        Self::start_with_options(
            bytes,
            FixtureServerOptions { truncate_after_absolute: Some(truncate_after_absolute) },
        )
    }

    fn start_with_options(bytes: Vec<u8>, options: FixtureServerOptions) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("failed to bind fixture server: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to set nonblocking listener: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("failed to read fixture server addr: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let bytes = Arc::new(bytes);
        let server_bytes = Arc::clone(&bytes);
        let options = Arc::new(options);
        let server_options = Arc::clone(&options);

        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle_fixture_connection(stream, &server_bytes, *server_options);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { addr, stop, handle: Some(handle) })
    }

    fn url(&self) -> String {
        format!("http://{}/fixture.mp3", self.addr)
    }
}

impl Drop for StaticAudioServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr).and_then(|stream| stream.shutdown(Shutdown::Both));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_fixture_connection(
    mut stream: TcpStream,
    bytes: &[u8],
    options: FixtureServerOptions,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("set_read_timeout failed: {e}"))?;

    let mut reader = BufReader::new(
        stream.try_clone().map_err(|e| format!("clone stream failed: {e}"))?,
    );
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| format!("read request line failed: {e}"))?;

    if request_line.trim().is_empty() {
        return Ok(());
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| "missing request method".to_string())?;
    let path = parts.next().ok_or_else(|| "missing request path".to_string())?;

    let mut range_header = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("read header line failed: {e}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Range: ") {
            range_header = Some(value.to_string());
        }
    }

    if path != "/fixture.mp3" {
        write_fixture_response(&mut stream, "404 Not Found", &[], None, method == "HEAD")?;
        return Ok(());
    }

    handle_range_request(&mut stream, method, range_header, bytes, options)
}

fn handle_range_request(
    stream: &mut TcpStream,
    method: &str,
    range_header: Option<String>,
    bytes: &[u8],
    options: FixtureServerOptions,
) -> Result<(), String> {
    let (status, mut body, mut content_range, body_start) =
        match parse_range_request(range_header.as_deref(), bytes.len()) {
            Ok(Some((start, end))) => (
                "206 Partial Content",
                &bytes[start..end],
                Some(format!("bytes {}-{}/{}", start, end - 1, bytes.len())),
                start,
            ),
            Ok(None) => ("200 OK", bytes, None, 0),
        Err(()) => {
            write_fixture_response(
                stream,
                "416 Range Not Satisfiable",
                &[],
                Some(format!("bytes */{}", bytes.len())),
                method == "HEAD",
            )?;
            return Ok(());
        }
    };

    if let Some(truncate_after_absolute) = options.truncate_after_absolute {
        if body_start < truncate_after_absolute {
            let max_len = truncate_after_absolute - body_start;
            body = &body[..body.len().min(max_len)];
        } else {
            body = &[];
        }

        if content_range.is_some() {
            content_range = if body.is_empty() {
                None
            } else {
                let end = body_start + body.len() - 1;
                Some(format!("bytes {}-{}/{}", body_start, end, bytes.len()))
            };
        }
    }

    write_fixture_response(stream, status, body, content_range, method == "HEAD")
}

fn parse_range_request(header: Option<&str>, len: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(header) = header else {
        return Ok(None);
    };

    let spec = header.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') {
        return Err(());
    }

    let (start, end) = spec.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = len.saturating_sub(suffix.min(len));
        return Ok(Some((start, len)));
    }

    let start = start.parse::<usize>().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }

    if end.is_empty() {
        return Ok(Some((start, len)));
    }

    let end = end.parse::<usize>().map_err(|_| ())?;
    if end < start {
        return Err(());
    }

    Ok(Some((start, end.saturating_add(1).min(len))))
}

fn write_fixture_response(
    stream: &mut TcpStream,
    status: &str,
    body: &[u8],
    content_range: Option<String>,
    head_only: bool,
) -> Result<(), String> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n",
        body.len()
    );

    if let Some(content_range) = content_range {
        response.push_str(&format!("Content-Range: {content_range}\r\n"));
    }

    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .map_err(|e| format!("write response headers failed: {e}"))?;
    if !head_only {
        stream.write_all(body).map_err(|e| format!("write response body failed: {e}"))?;
    }
    stream.flush().map_err(|e| format!("flush response failed: {e}"))?;
    Ok(())
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

fn fixture_wav_bytes(duration_secs: u32, sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 8;
    let bytes_per_sample = (bits_per_sample / 8) as u32;
    let data_len = duration_secs
        .saturating_mul(sample_rate)
        .saturating_mul(channels as u32)
        .saturating_mul(bytes_per_sample);
    let riff_len = 36u32.saturating_add(data_len);
    let byte_rate = sample_rate
        .saturating_mul(channels as u32)
        .saturating_mul(bytes_per_sample);
    let block_align = channels * (bits_per_sample / 8);

    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_len.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(bytes.len() + data_len as usize, 128u8);
    bytes
}

fn wait_for_end_file_reason(
    internal_events: &Receiver<InternalEvent>,
    timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        match internal_events.recv_timeout(Duration::from_millis(200)) {
            Ok(InternalEvent::EndFile { reason }) => return Ok(reason),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    Err(format!("timed out waiting for EndFile event after {timeout:?}"))
}

fn init_test_logger() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .is_test(true)
            .try_init();
    });
}

struct StaticUrlPreparer {
    url: String,
    prepare_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl MediaPreparer for StaticUrlPreparer {
    async fn prepare(&self, _track_id: &str, _tier: PreloadTier) -> anyhow::Result<PreparedMedia> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        Ok(PreparedMedia::Direct { url: self.url.clone() })
    }

    fn prefetch(&self, _track_id: &str, _tier: PreloadTier) {}
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
#[ignore = "requires mpv"]
fn test_runtime_direct_builder_path_starts_fixture_playback() {
    let fixture_bytes = fixture_mp3_bytes();
    let server = StaticAudioServer::start(fixture_bytes).expect("fixture server should start");
    let (_temp_dir, playback) =
        build_playback_service(AudioDeliveryMode::Direct).expect("playback service should start");

    let prepared = PreparedMedia::Direct { url: server.url() };
    let input = FfmpegConcatSource::build_from_prepared(&prepared).expect("builder should succeed");

    assert_eq!(input.url, server.url());
    assert!(input.mpv_args.is_empty());

    playback.playlist_append_input(&input).expect("append input should succeed");
    playback.playlist_play_index(0).expect("play index should succeed");

    let ttfa = wait_for_first_audio(&playback, Duration::from_secs(5))
        .expect("direct runtime should reach first audio");
    let steady_position = wait_for_position_at_least(&playback, 1.0, Duration::from_secs(5))
        .expect("direct runtime should stay active through sample window");

    println!(
        "runtime_direct_fixture ttfa_ms={} steady_position_s={steady_position:.3}",
        ttfa.as_millis()
    );
}

#[test]
#[ignore = "requires mpv"]
fn test_runtime_combined_builder_path_starts_fixture_playback() {
    let fixture_bytes = fixture_mp3_bytes();
    let fixture_len = fixture_bytes.len() as u64;
    let prefix_len = fixture_len.min(64 * 1024) as usize;
    let prefix_dir = TempDir::new().expect("tempdir should be created");
    let prefix_path = prefix_dir.path().join("fixture-prefix.mp3");
    fs::write(&prefix_path, &fixture_bytes[..prefix_len]).expect("prefix should be written");

    let server = StaticAudioServer::start(fixture_bytes).expect("fixture server should start");
    let (_temp_dir, playback) = build_playback_service(AudioDeliveryMode::Combined)
        .expect("combined playback service should start");

    let prepared = PreparedMedia::StagedPrefix {
        path: prefix_path.clone(),
        bytes: prefix_len as u64,
        url: server.url(),
        content_length: fixture_len,
    };
    let input = FfmpegConcatSource::build_from_prepared(&prepared).expect("builder should succeed");

    assert!(input.url.starts_with("lavf://concat:"));
    assert_eq!(
        input.mpv_args,
        vec![
            "--stream-lavf-o-append=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat"
                .to_string()
        ]
    );

    playback.playlist_append_input(&input).expect("append input should succeed");
    playback.playlist_play_index(0).expect("play index should succeed");

    let ttfa = wait_for_first_audio(&playback, Duration::from_secs(5))
        .expect("combined runtime should reach first audio");
    let steady_position = wait_for_position_at_least(&playback, 1.0, Duration::from_secs(5))
        .expect("combined runtime should stay active through sample window");

    println!(
        "runtime_combined_fixture ttfa_ms={} steady_position_s={steady_position:.3} prefix_bytes={}",
        ttfa.as_millis(),
        prefix_len
    );
}

#[test]
#[ignore = "requires mpv"]
fn test_runtime_direct_truncated_fixture_triggers_eof_recovery_diagnostic() {
    init_test_logger();

    const SAMPLE_RATE: u32 = 8_000;
    const FULL_DURATION_SECS: u32 = 120;
    const TRUNCATE_AT_SECS: u32 = 40;
    const SEEK_TARGET_SECS: f64 = 35.0;

    let fixture_bytes = fixture_wav_bytes(FULL_DURATION_SECS, SAMPLE_RATE);
    let truncate_after_absolute = 44 + (TRUNCATE_AT_SECS * SAMPLE_RATE) as usize;

    let server = StaticAudioServer::start_with_truncation(fixture_bytes, truncate_after_absolute)
        .expect("fixture server should start with truncation");
    let (temp_dir, playback) =
        build_playback_service(AudioDeliveryMode::Direct).expect("playback service should start");
    let playback = Arc::new(playback);

    let queue = Arc::new(QueueService::new());
    let mut song = Song::default();
    song.uri = "fixture-eof-track".to_string();
    song.metadata.insert("title".to_string(), vec!["fixture-eof-track".to_string()]);
    queue.add(song, None);
    let state_tracker = Arc::new(PlaybackStateTracker::new());

    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let media_preparer: Arc<dyn MediaPreparer> = Arc::new(StaticUrlPreparer {
        url: server.url(),
        prepare_calls: Arc::clone(&prepare_calls),
    });

    let socket_path = temp_dir.path().join("mpv.sock");
    let (event_tx, _event_rx) = channel::unbounded::<String>();
    let (internal_event_tx, internal_event_rx) = channel::unbounded::<InternalEvent>();
    playback
        .start_event_loop(&socket_path, event_tx, internal_event_tx)
        .expect("event loop should start");

    let response = orchestrator::play_position_sync(
        &playback,
        &queue,
        0,
        &state_tracker,
        &media_preparer,
    );
    assert!(matches!(response, rmpc::backends::youtube::protocol::ServerResponse::Ok));

    wait_for_first_audio(playback.as_ref(), Duration::from_secs(5))
        .expect("fixture should reach first audio");
    playback.seek(SEEK_TARGET_SECS, "absolute").expect("seek should succeed");
    let observed_position = wait_for_position_at_least(
        playback.as_ref(),
        SEEK_TARGET_SECS,
        Duration::from_secs(5),
    )
        .expect("seek target should be reached");
    let duration = playback.get_duration().expect("duration should be readable");

    let reason = wait_for_end_file_reason(&internal_event_rx, Duration::from_secs(10))
        .expect("expected EndFile event");
    assert_eq!(reason, "eof", "fixture truncation should end with EOF");

    let eof_position = observed_position;
    let remaining = duration - eof_position;
    let intended_repro_window = duration >= 90.0
        && eof_position >= 30.0
        && eof_position < duration
        && remaining >= 20.0;
    assert!(
        intended_repro_window,
        "expected known repro timing window, got {eof_position:.2}/{duration:.2}"
    );

    let eof_position_err = playback
        .get_position()
        .expect_err("EOF boundary should expose unavailable time-pos for this repro")
        .to_string();
    assert!(
        eof_position_err.contains("property unavailable"),
        "expected explicit EOF cause, got: {eof_position_err}"
    );

    orchestrator::handle_track_ended(&playback, &queue, &state_tracker, &media_preparer, &reason);

    let calls = prepare_calls.load(Ordering::SeqCst);
    let state = state_tracker.get();
    assert_eq!(calls, 1, "expected no replay when EOF cause is explicit; prepare calls={calls}");
    assert!(matches!(state, PlaybackState::PendingAdvance { .. }));

    println!(
        "[DIAG-EOF-TEST] reason={reason} eof_position_s={eof_position:.2} duration_s={duration:.2} remaining_s={remaining:.2} intended_repro_window={intended_repro_window} explicit_cause={eof_position_err:?} recovery_prepare_calls={calls} state={state:?}"
    );
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
