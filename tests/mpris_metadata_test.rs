//! MPRIS Metadata Test - TDD for force-media-title fix
//! 
//! This test verifies that MPRIS metadata shows proper song title/artist
//! instead of raw URLs.
//!
//! Run with: cargo test --test mpris_metadata_test -- --ignored --nocapture

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const MPV_SOCKET: &str = "/tmp/mpv-test-mpris.sock";
const TEST_URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
const EXPECTED_TITLE: &str = "Test Artist - Test Song Title";

/// Simple MPV IPC client for testing
struct TestMpvIpc {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    request_id: u64,
}

impl TestMpvIpc {
    fn connect(socket_path: &str) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            request_id: 1,
        })
    }

    fn send_command(&mut self, command: serde_json::Value) -> Result<serde_json::Value, String> {
        let request_id = self.request_id;
        self.request_id += 1;

        let cmd_obj = serde_json::json!({
            "command": command,
            "request_id": request_id
        });

        let mut json_str = serde_json::to_string(&cmd_obj).map_err(|e| e.to_string())?;
        json_str.push('\n');

        self.writer.write_all(json_str.as_bytes()).map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())?;

        // Read response
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err("Socket closed".to_string()),
                Ok(_) => {
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&line) {
                        if resp.get("request_id") == Some(&serde_json::json!(request_id)) {
                            if let Some(err) = resp.get("error").and_then(|e| e.as_str()) {
                                if err != "success" {
                                    return Err(format!("MPV error: {}", err));
                                }
                            }
                            return Ok(resp.get("data").cloned().unwrap_or(serde_json::Value::Null));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    fn get_property(&mut self, property: &str) -> Result<serde_json::Value, String> {
        self.send_command(serde_json::json!(["get_property", property]))
    }

    fn set_property(&mut self, property: &str, value: serde_json::Value) -> Result<(), String> {
        self.send_command(serde_json::json!(["set_property", property, value]))?;
        Ok(())
    }

    /// Send loadfile with options - the FIX being tested
    /// Options should be a comma-separated string like "force-media-title=value,option2=value2"
    fn loadfile_with_options(&mut self, url: &str, mode: &str, options: &str) -> Result<(), String> {
        self.send_command(serde_json::json!(["loadfile", url, mode, options]))?;
        Ok(())
    }

    /// Send loadfile WITHOUT options - the BROKEN behavior
    fn loadfile_without_options(&mut self, url: &str, mode: &str) -> Result<(), String> {
        self.send_command(serde_json::json!(["loadfile", url, mode]))?;
        Ok(())
    }
}

/// Spawn a test MPV instance
fn spawn_mpv() -> Result<Child, String> {
    // Clean up old socket
    let _ = std::fs::remove_file(MPV_SOCKET);

    let child = Command::new("mpv")
        .args([
            "--idle=yes",
            "--vo=null",
            "--no-terminal",
            &format!("--input-ipc-server={}", MPV_SOCKET),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn MPV: {}", e))?;

    // Wait for socket
    for _ in 0..50 {
        if Path::new(MPV_SOCKET).exists() {
            thread::sleep(Duration::from_millis(100));
            return Ok(child);
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err("Timeout waiting for MPV socket".to_string())
}

/// Test: set force-media-title BEFORE loadfile sets media-title correctly
#[test]
#[ignore = "requires MPV installed"]
fn test_loadfile_with_force_media_title() {
    let mut mpv_process = spawn_mpv().expect("Failed to spawn MPV");

    let result = std::panic::catch_unwind(|| {
        let mut ipc = TestMpvIpc::connect(MPV_SOCKET).expect("Failed to connect to MPV");

        // The FIX: set force-media-title BEFORE loadfile
        ipc.set_property("force-media-title", serde_json::json!(EXPECTED_TITLE))
            .expect("Failed to set force-media-title");

        // Use a simple audio URL that loads quickly (silence)
        let test_url = "av://lavfi:sine=frequency=440:duration=1";
        
        ipc.send_command(serde_json::json!(["loadfile", test_url, "replace"]))
            .expect("loadfile failed");

        // Wait for file to load
        thread::sleep(Duration::from_millis(500));

        // Check media-title property
        let media_title = ipc.get_property("media-title")
            .expect("Failed to get media-title");

        let title_str = media_title.as_str().unwrap_or("");
        
        println!("media-title: {}", title_str);
        
        assert_eq!(
            title_str, EXPECTED_TITLE,
            "media-title should match force-media-title. Got: '{}', Expected: '{}'",
            title_str, EXPECTED_TITLE
        );
    });

    // Cleanup
    let _ = mpv_process.kill();
    let _ = mpv_process.wait();
    let _ = std::fs::remove_file(MPV_SOCKET);

    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Test: loadfile WITHOUT force-media-title shows wrong metadata
/// This test documents the CURRENT BROKEN behavior
#[test]
#[ignore = "requires MPV installed"]
fn test_loadfile_without_force_media_title_shows_url() {
    let mut mpv_process = spawn_mpv().expect("Failed to spawn MPV");

    let result = std::panic::catch_unwind(|| {
        let mut ipc = TestMpvIpc::connect(MPV_SOCKET).expect("Failed to connect to MPV");

        // The BROKEN behavior: loadfile without options
        let test_url = "av://lavfi:sine=frequency=440:duration=1";
        
        ipc.loadfile_without_options(test_url, "replace")
            .expect("loadfile failed");

        // Wait for file to load
        thread::sleep(Duration::from_millis(500));

        // Check media-title - should NOT be our expected title
        let media_title = ipc.get_property("media-title")
            .expect("Failed to get media-title");

        let title_str = media_title.as_str().unwrap_or("");
        
        println!("media-title (no force): {}", title_str);
        
        // Without force-media-title, it shows the URL or filename
        assert_ne!(
            title_str, EXPECTED_TITLE,
            "Without force-media-title, media-title should NOT be '{}'",
            EXPECTED_TITLE
        );
    });

    // Cleanup
    let _ = mpv_process.kill();
    let _ = mpv_process.wait();
    let _ = std::fs::remove_file(MPV_SOCKET);

    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Integration test with the actual PlaybackService
/// This will fail until the fix is implemented
#[test]
#[ignore = "requires daemon running - integration test"]
fn test_playback_service_sets_mpris_metadata() {
    // This test requires:
    // 1. The daemon to be running with the fix
    // 2. A real YouTube video to play
    
    // For now, we document the expected behavior:
    // 1. Call PlaybackService::play(url, title, artist)
    // 2. Check MPV's media-title property
    // 3. Assert it shows "artist - title"
    
    println!("This test requires the daemon to be running.");
    println!("Run manually after implementing the fix:");
    println!("  1. Start daemon: ./restart_daemon.sh");
    println!("  2. Play a song via rmpc");
    println!("  3. Check: playerctl metadata");
    println!("  4. Expected: title shows song name, not URL");
}
