//! Integration test for YouTube backend playback
//! Tests the stream URL extraction and MPV playback flow

use std::time::Duration;

/// Test that searches for a video and plays it, checking for HTTP 400 errors
/// This test runs the full playback flow outside of the TUI
#[test]
#[ignore = "requires network and YouTube cookies"]
fn test_youtube_playback_no_http_400() {
    // This test would require access to the YouTube backend internals
    // For now, we'll use a subprocess test approach
    
    let output = std::process::Command::new("cargo")
        .args(["run", "--release", "--bin", "rmpc", "--", "debugtest"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "info")
        .output()
        .expect("Failed to run debugtest");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", stdout, stderr);
    
    // Check for HTTP 400 error
    assert!(
        !combined.contains("status: 400") && !combined.contains("HTTP 400"),
        "HTTP 400 error detected in output:\n{}", combined
    );
    
    // Check for empty URL (stream extraction failure)
    assert!(
        !combined.contains("Got stream URL (length=0)"),
        "Empty URL - stream extraction failed:\n{}", combined
    );
    
    // Check for successful playback
    assert!(
        combined.contains("OK: Audio playing") || combined.contains("playlist_entry_id"),
        "Playback did not succeed:\n{}", combined
    );
}

/// Test stream URL extraction directly
#[test]
#[ignore = "requires network and YouTube cookies"]
fn test_stream_url_extraction() {
    // Run a quick test to verify stream extraction works
    let output = std::process::Command::new("cargo")
        .args(["run", "--release", "--bin", "rmpc", "--", "debugtest"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "debug")
        .output()
        .expect("Failed to run debugtest");
    
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    // Should contain successful URL extraction
    assert!(
        stderr.contains("Got stream URL (length=") && !stderr.contains("length=0)"),
        "Stream URL extraction failed or returned empty URL:\n{}", stderr
    );
    
    // URL should be a valid googlevideo.com URL
    assert!(
        stderr.contains("googlevideo.com"),
        "Stream URL should be from googlevideo.com:\n{}", stderr
    );
}
