//! Integration tests for YouTube playback
//! Run with: cargo test --test playback_test -- --ignored --nocapture

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Runs debugtest and captures output until playback starts, then kills it
fn run_debugtest_until_playing() -> (String, String, bool) {
    let mut child = Command::new("cargo")
        .args(["run", "--release", "--bin", "rmpc", "--", "debugtest"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn debugtest");

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_lines = Vec::new();
    let mut stderr_lines = Vec::new();
    let mut success = false;

    // Read stdout in a thread
    let stdout_handle = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut lines = Vec::new();
        for line in reader.lines().map_while(Result::ok) {
            lines.push(line.clone());
            if line.contains("OK: Audio playing") {
                break;
            }
            if line.contains("Press Enter") {
                break;
            }
        }
        lines
    });

    // Read stderr
    let stderr_handle = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        reader.lines().map_while(Result::ok).collect::<Vec<_>>()
    });

    // Wait for stdout to finish (max 30s)
    stdout_lines = stdout_handle.join().unwrap_or_default();
    
    // Give stderr a moment then collect
    std::thread::sleep(Duration::from_millis(500));
    
    // Kill the process
    let _ = child.kill();
    let _ = child.wait();
    
    stderr_lines = stderr_handle.join().unwrap_or_default();

    let stdout_str = stdout_lines.join("\n");
    let stderr_str = stderr_lines.join("\n");
    success = stdout_str.contains("OK: Audio playing");

    (stdout_str, stderr_str, success)
}

/// Test that debugtest command works and doesn't produce HTTP 400 errors
#[test]
#[ignore = "requires network and cookies"]
fn test_debugtest_no_http_400() {
    let (stdout, stderr, success) = run_debugtest_until_playing();
    let combined = format!("{stdout}\n{stderr}");

    println!("=== stdout ===\n{stdout}");
    println!("=== stderr ===\n{stderr}");

    // Should NOT have HTTP 400 errors
    assert!(
        !combined.contains("status: 400"),
        "HTTP 400 error in output:\n{combined}"
    );
    assert!(
        !combined.contains("Bad Request"),
        "Bad Request error in output:\n{combined}"
    );

    // Should have successful stream URL extraction
    assert!(
        stderr.contains("Got stream URL (length=") && !stderr.contains("length=0)"),
        "Stream URL extraction failed:\n{combined}"
    );

    // Should have successful playback
    assert!(success, "Playback failed:\n{combined}");
}

/// Test stream URL extraction with multiple runs to detect intermittent failures
#[test]
#[ignore = "requires network and cookies"]  
fn test_multiple_runs_no_http_400() {
    for i in 1..=3 {
        println!("=== Test iteration {}/3 ===", i);
        let (stdout, stderr, success) = run_debugtest_until_playing();
        let combined = format!("{stdout}\n{stderr}");

        if combined.contains("status: 400") || combined.contains("Bad Request") {
            panic!("HTTP 400 error on iteration {}:\n{}", i, combined);
        }

        if !success {
            panic!("Playback failed on iteration {}:\n{}", i, combined);
        }

        println!("Iteration {} passed", i);
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Verify stream URL is from googlevideo.com (indicates proper client usage)
#[test]
#[ignore = "requires network"]
fn test_stream_url_is_googlevideo() {
    let (_, stderr, _) = run_debugtest_until_playing();
    
    assert!(
        stderr.contains("googlevideo.com"),
        "Stream URL should be from googlevideo.com:\n{stderr}"
    );
}
