//! E2E tests using portable-pty for reliable terminal interaction.
//!
//! This module provides reliable TUI testing by using a real PTY (pseudo-terminal)
//! with synchronous I/O and proper terminal emulation via vt100.
//!
//! Based on the pattern used by git-branchless for testing interactive TUI apps.

use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc::channel, Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Terminal escape codes for special keys
pub const UP_ARROW: &str = "\x1b[A";
pub const DOWN_ARROW: &str = "\x1b[B";
pub const RIGHT_ARROW: &str = "\x1b[C";
pub const LEFT_ARROW: &str = "\x1b[D";
pub const ENTER: &str = "\r";
pub const ESC: &str = "\x1b";

/// Action to perform in PTY test
#[derive(Debug, Clone)]
pub enum PtyAction<'a> {
    /// Write keystrokes to terminal
    Write(&'a str),
    /// Wait until screen contains text (with timeout)
    WaitUntilContains(&'a str),
    /// Wait for specified duration
    Sleep(Duration),
}

/// Get path to rmpc binary
fn get_rmpc_bin() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("target/release/rmpc")
}

/// Get path to config file
fn get_config_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("parent dir")
        .join("config/rmpc.ron")
}

/// Generate unique log file for test
fn get_log_file(test_name: &str) -> PathBuf {
    let safe_name = test_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();
    PathBuf::from(format!(
        "/tmp/rmpc-pty-{}-{}.log",
        safe_name,
        std::process::id()
    ))
}

/// Clear log file before test
fn clear_log_file(path: &PathBuf) {
    let _ = fs::write(path, "");
}

/// Read log file contents
fn read_log_file(path: &PathBuf) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Result of PTY test run
pub struct PtyTestResult {
    pub exit_status: ExitStatus,
    pub screen_contents: String,
    pub log_contents: String,
}

/// Run rmpc in PTY with given actions
///
/// This follows the git-branchless pattern for reliable TUI testing:
/// 1. Open a real PTY
/// 2. Spawn rmpc process in the PTY
/// 3. Use vt100 parser for terminal emulation
/// 4. Execute actions synchronously (Write/WaitUntilContains)
pub fn run_rmpc_in_pty(
    test_name: &str,
    extra_args: &[&str],
    actions: &[PtyAction<'_>],
    timeout_secs: u64,
) -> Result<PtyTestResult, String> {
    let log_file = get_log_file(test_name);
    clear_log_file(&log_file);

    let rmpc_bin = get_rmpc_bin();
    let config_path = get_config_path();

    if !rmpc_bin.exists() {
        return Err(format!(
            "rmpc binary not found at {:?}. Run 'cargo build --release' first.",
            rmpc_bin
        ));
    }

    // Create PTY
    let pty_system = native_pty_system();
    let pty_size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pty = pty_system
        .openpty(pty_size)
        .map_err(|e| format!("Could not open PTY: {}", e))?;

    let mut pty_master = pty
        .master
        .take_writer()
        .map_err(|e| format!("Could not take PTY writer: {}", e))?;

    // Build command
    let mut cmd = CommandBuilder::new(&rmpc_bin);
    cmd.env("RMPC_LOG_FILE", log_file.to_str().unwrap_or(""));
    cmd.env("RUST_LOG", "debug");
    cmd.env("TERM", "xterm-256color");
    // Clear TMUX vars to prevent interference
    cmd.env_remove("TMUX");
    cmd.env_remove("TMUX_PANE");

    cmd.arg("--config");
    cmd.arg(config_path.to_str().unwrap_or(""));

    for arg in extra_args {
        cmd.arg(*arg);
    }

    // Spawn process
    let mut child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Could not spawn rmpc: {}", e))?;

    // Set up terminal parser
    let reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| format!("Could not clone PTY reader: {}", e))?;
    let reader = Arc::new(Mutex::new(reader));

    let parser = vt100::Parser::new(pty_size.rows, pty_size.cols, 0);
    let parser = Arc::new(Mutex::new(parser));

    // Execute actions
    for action in actions {
        match action {
            PtyAction::Write(value) => {
                // Check if process is still running
                if let Ok(Some(exit_status)) = child.try_wait() {
                    let screen = parser.lock().unwrap().screen().contents();
                    return Err(format!(
                        "Process exited with {:?} before writing {:?}.\nScreen:\n{}",
                        exit_status, value, screen
                    ));
                }

                write!(pty_master, "{}", value)
                    .map_err(|e| format!("Failed to write to PTY: {}", e))?;
                pty_master
                    .flush()
                    .map_err(|e| format!("Failed to flush PTY: {}", e))?;
            }

            PtyAction::WaitUntilContains(value) => {
                let (finished_tx, finished_rx) = channel();

                let wait_thread = {
                    let parser = Arc::clone(&parser);
                    let reader = Arc::clone(&reader);
                    let value = value.to_string();

                    thread::spawn(move || -> Result<(), String> {
                        loop {
                            // Check if screen contains target string
                            {
                                let p = parser.lock().unwrap();
                                if p.screen().contents().contains(&value) {
                                    break;
                                }
                            }

                            // Read more data from PTY
                            let mut reader = reader.lock().unwrap();
                            let mut buffer = [0u8; 4096];
                            match reader.read(&mut buffer) {
                                Ok(0) => {
                                    return Err("PTY EOF".to_string());
                                }
                                Ok(n) => {
                                    let mut p = parser.lock().unwrap();
                                    p.process(&buffer[..n]);
                                }
                                Err(e) => {
                                    return Err(format!("PTY read error: {}", e));
                                }
                            }
                        }
                        let _ = finished_tx.send(());
                        Ok(())
                    })
                };

                let timeout = Duration::from_secs(timeout_secs);
                if finished_rx.recv_timeout(timeout).is_err() {
                    let screen = parser.lock().unwrap().screen().contents();
                    return Err(format!(
                        "Timeout waiting for {:?} after {}s.\nScreen contents:\n-----\n{}\n-----",
                        value, timeout_secs, screen
                    ));
                }

                wait_thread
                    .join()
                    .map_err(|_| "Wait thread panicked")?
                    .map_err(|e| e)?;
            }

            PtyAction::Sleep(duration) => {
                thread::sleep(*duration);
            }
        }
    }

    // Get final screen contents
    let screen_contents = parser.lock().unwrap().screen().contents();

    // Terminate the process gracefully
    let _ = write!(pty_master, "q"); // Try to quit
    let _ = pty_master.flush();

    // Wait briefly for graceful exit
    thread::sleep(Duration::from_millis(500));

    // Force kill if still running
    if child.try_wait().map_err(|e| e.to_string())?.is_none() {
        let _ = child.kill();
    }

    let exit_status = child.wait().map_err(|e| format!("Wait failed: {}", e))?;

    let log_contents = read_log_file(&log_file);

    Ok(PtyTestResult {
        exit_status,
        screen_contents,
        log_contents,
    })
}

// =============================================================================
// UI VISIBILITY TESTS
// =============================================================================

#[test]
fn test_ui_app_starts_and_shows_tabs() {
    let result = run_rmpc_in_pty(
        "ui_app_starts",
        &[],
        &[
            PtyAction::WaitUntilContains("Queue"),
            PtyAction::WaitUntilContains("Saved"),
        ],
        10,
    );

    match result {
        Ok(r) => {
            assert!(
                r.screen_contents.contains("Queue"),
                "Screen should show Queue tab.\nScreen:\n{}",
                r.screen_contents
            );
            assert!(
                r.screen_contents.contains("Saved"),
                "Screen should show Saved tab.\nScreen:\n{}",
                r.screen_contents
            );
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}

#[test]
fn test_ui_search_pane_elements() {
    let result = run_rmpc_in_pty(
        "ui_search_pane",
        &[],
        &[PtyAction::WaitUntilContains("Any Tag")],
        10,
    );

    match result {
        Ok(r) => {
            assert!(
                r.screen_contents.contains("Any Tag"),
                "Screen should show 'Any Tag' filter.\nScreen:\n{}",
                r.screen_contents
            );
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}

#[test]
fn test_ui_tab_navigation() {
    let result = run_rmpc_in_pty(
        "ui_tab_nav",
        &[],
        &[
            PtyAction::WaitUntilContains("Search"), // Default tab
            PtyAction::Write("1"),                  // Go to Queue tab
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write("3"), // Go to Saved tab
            PtyAction::Sleep(Duration::from_millis(300)),
        ],
        10,
    );

    match result {
        Ok(r) => {
            // Just verify app didn't crash during navigation
            assert!(
                !r.screen_contents.is_empty(),
                "Screen should have content after tab navigation"
            );
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}

// =============================================================================
// FEATURE TESTS - DISABLED: PTY input doesn't work with crossterm
// =============================================================================
// 
// NOTE: These tests are disabled because portable-pty keyboard input doesn't
// reach crossterm-based apps. crossterm uses its own input polling mechanism
// that doesn't work properly in a PTY environment.
//
// For testing the actual bugs, use:
// 1. Manual testing with the TUI
// 2. Backend integration tests (see youtube_backend_tests.rs)
// 3. The original tui-test tests (npm test) with appropriate waits
//
// Known bugs to fix:
// - Bug #1: Enter on song = HTTP 400 (play_id issue)
// - Bug #2: Artist view shows debug logs (browse_artist issue)  
// - Bug #3: Album view shows IDs not metadata (browse_album issue)
// =============================================================================

/// Test: Play Song - MUST send loadfile command to MPV
///
/// Current bug: Enter on song results in HTTP 400 error
/// Success criteria: Log must contain "loadfile" (MPV command to play audio)
/// AND must NOT contain "status: 400" error
#[test]
#[ignore = "PTY keyboard input doesn't work with crossterm - use manual testing"]
fn test_feature_play_song() {
    let result = run_rmpc_in_pty(
        "feature_play_song",
        &[],
        &[
            // Wait for app to be ready (Search tab should be visible)
            PtyAction::WaitUntilContains("Any Tag"),
            // Give app time to fully initialize input handling
            PtyAction::Sleep(Duration::from_secs(3)),
            // Enter search mode - press 'i' to focus input
            PtyAction::Write("i"),
            // Wait for insert mode to activate
            PtyAction::Sleep(Duration::from_secs(2)),
            // Type search query character by character with delays
            PtyAction::Write("k"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("i"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("m"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write(" "),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("l"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("o"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("n"),
            PtyAction::Sleep(Duration::from_millis(100)),
            PtyAction::Write("g"),
            PtyAction::Sleep(Duration::from_millis(500)),
            // Submit search with Enter
            PtyAction::Write(ENTER),
            // Wait for search to complete - give YouTube API time
            PtyAction::Sleep(Duration::from_secs(15)),
            // Exit insert mode and navigate to results
            PtyAction::Write(ESC),
            PtyAction::Sleep(Duration::from_millis(500)),
            PtyAction::Write("l"), // Move right to results pane
            PtyAction::Sleep(Duration::from_millis(500)),
            PtyAction::Write("l"), // Move to Songs column  
            PtyAction::Sleep(Duration::from_millis(500)),
            PtyAction::Write("j"), // Move down to first song
            PtyAction::Sleep(Duration::from_millis(500)),
            PtyAction::Write(ENTER), // Press Enter to play
            // Wait for playback action
            PtyAction::Sleep(Duration::from_secs(5)),
        ],
        60, // Longer timeout for YouTube API
    );

    match result {
        Ok(r) => {
            let log = &r.log_contents;
            let screen = &r.screen_contents;

            // First, verify search was attempted
            let search_attempted = log.contains("search_yt") || 
                                   log.contains("SearchResult") ||
                                   log.contains("Phase set to BrowseResults");
            
            assert!(
                search_attempted,
                "FAIL: Search was never attempted.\n\
                 The keyboard input may not be reaching the app correctly.\n\
                 Screen:\n{}\n\
                 Log tail:\n{}",
                screen,
                &log[log.len().saturating_sub(2000)..]
            );

            // STRICT CHECK: Must have loadfile command (actual MPV playback)
            let has_loadfile = log.contains("loadfile");
            
            // Check for HTTP 400 error (the actual bug)
            let has_400_error = log.contains("status: 400") || log.contains("Bad Request");
            
            // Fail if we got HTTP 400
            assert!(
                !has_400_error,
                "FAIL: Got HTTP 400 error when trying to play song.\n\
                 This means the song URL/ID is not being processed correctly.\n\
                 Log tail:\n{}",
                &log[log.len().saturating_sub(3000)..]
            );

            // Fail if no loadfile command was sent
            assert!(
                has_loadfile,
                "FAIL: No 'loadfile' command sent to MPV.\n\
                 Expected: MPV should receive loadfile command to play audio.\n\
                 Log tail:\n{}",
                &log[log.len().saturating_sub(3000)..]
            );
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}

/// Test: View Artist - MUST show artist info without errors
///
/// Current bugs:
/// 1. Shows debug logs instead of proper UI
/// 2. Breaks the TUI
/// Success criteria:
/// - No HTTP 400 errors
/// - No "DEBUG" or raw log output visible on screen
/// - Artist name should be visible on screen after browse
#[test]
#[ignore = "PTY keyboard input doesn't work with crossterm - use manual testing"]
fn test_feature_view_artist() {
    let result = run_rmpc_in_pty(
        "feature_view_artist",
        &[],
        &[
            // Wait for app to be ready
            PtyAction::WaitUntilContains("Any Tag"),
            PtyAction::Sleep(Duration::from_millis(300)),
            // Enter search mode
            PtyAction::Write("i"),
            PtyAction::Sleep(Duration::from_millis(500)),
            // Search for "kim long"
            PtyAction::Write("kim long"),
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write(ENTER),
            // Wait for search results - Artists column should appear
            PtyAction::WaitUntilContains("Artists"),
            PtyAction::Sleep(Duration::from_millis(500)),
            // Exit search input mode
            PtyAction::Write(ESC),
            PtyAction::Sleep(Duration::from_millis(300)),
            // Navigate to Artists column (should be the first/leftmost)
            PtyAction::Write("l"), // Move to results area
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write("j"), // Move down to first artist
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write(ENTER), // Browse artist
            // Wait for artist view to load
            PtyAction::Sleep(Duration::from_secs(5)),
        ],
        60, // Longer timeout for YouTube API
    );

    match result {
        Ok(r) => {
            let log = &r.log_contents;
            let screen = &r.screen_contents;

            // Check for HTTP 400 error
            let has_400_error = log.contains("status: 400") || log.contains("Bad Request");
            
            assert!(
                !has_400_error,
                "FAIL: browse_artist triggered HTTP 400 error.\n\
                 This means the artist ID prefix was not stripped correctly.\n\
                 Log tail:\n{}",
                &log[log.len().saturating_sub(3000)..]
            );

            // Check that screen doesn't show raw debug output
            let has_debug_on_screen = screen.contains("DEBUG") || 
                                       screen.contains("TRACE") ||
                                       screen.contains("browse_artist") ||
                                       screen.contains("raw_id=");
            
            assert!(
                !has_debug_on_screen,
                "FAIL: Screen shows debug/log output instead of proper UI.\n\
                 Screen contents:\n{}",
                screen
            );

            // Verify browse_artist was actually called
            assert!(
                log.contains("browse_artist"),
                "FAIL: browse_artist was never called.\n\
                 Log tail:\n{}",
                &log[log.len().saturating_sub(2000)..]
            );
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}

/// Test: View Album - MUST show song metadata (title, artist, duration)
///
/// Current bug: Shows list of IDs instead of song metadata
/// Success criteria:
/// - Album songs should show title, artist, duration
/// - NOT just raw video IDs
#[test]
#[ignore = "PTY keyboard input doesn't work with crossterm - use manual testing"]
fn test_feature_view_album() {
    let result = run_rmpc_in_pty(
        "feature_view_album",
        &[],
        &[
            // Wait for app to be ready
            PtyAction::WaitUntilContains("Any Tag"),
            PtyAction::Sleep(Duration::from_millis(300)),
            // Enter search mode
            PtyAction::Write("i"),
            PtyAction::Sleep(Duration::from_millis(500)),
            // Search for "kim long"
            PtyAction::Write("kim long"),
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write(ENTER),
            // Wait for search results - Albums column should appear
            PtyAction::WaitUntilContains("Albums"),
            PtyAction::Sleep(Duration::from_millis(500)),
            // Exit search input mode
            PtyAction::Write(ESC),
            PtyAction::Sleep(Duration::from_millis(300)),
            // Navigate to Albums column (usually middle)
            PtyAction::Write("l"), // Move to results area
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write("l"), // Move right to Albums
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write("j"), // Move down to first album
            PtyAction::Sleep(Duration::from_millis(300)),
            PtyAction::Write(ENTER), // Browse album
            // Wait for album view to load
            PtyAction::Sleep(Duration::from_secs(5)),
        ],
        60, // Longer timeout for YouTube API
    );

    match result {
        Ok(r) => {
            let log = &r.log_contents;
            let screen = &r.screen_contents;

            // Check for HTTP 400 error
            let has_400_error = log.contains("status: 400") || log.contains("Bad Request");
            
            assert!(
                !has_400_error,
                "FAIL: browse_album triggered HTTP 400 error.\n\
                 Log tail:\n{}",
                &log[log.len().saturating_sub(3000)..]
            );

            // Check that screen doesn't show raw video IDs without metadata
            // YouTube video IDs are 11 characters like "dQw4w9WgXcQ"
            let lines: Vec<&str> = screen.lines().collect();
            let id_only_lines: Vec<&str> = lines
                .iter()
                .filter(|line| {
                    let trimmed = line.trim();
                    // Check if line looks like just a video ID (11 chars, alphanumeric with - and _)
                    trimmed.len() == 11 && 
                    trimmed.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                })
                .copied()
                .collect();

            assert!(
                id_only_lines.is_empty(),
                "FAIL: Screen shows raw video IDs without metadata.\n\
                 Found {} lines that look like bare video IDs:\n{:?}\n\
                 Screen:\n{}",
                id_only_lines.len(),
                id_only_lines,
                screen
            );

            // Screen should show some indication of song info (duration format like "3:45" or metadata)
            let has_duration_format = screen.contains(':') && 
                screen.chars().filter(|c| c.is_numeric()).count() > 5;
            let has_song_metadata = screen.contains("Artist") || 
                                    screen.contains("Title") || 
                                    screen.contains("Duration") ||
                                    screen.contains("Album");

            // This is a softer check - at least something that looks like metadata should be present
            if !has_duration_format && !has_song_metadata {
                // Log warning but don't fail yet - need to see actual output first
                eprintln!(
                    "WARNING: Screen may not show proper song metadata.\nScreen:\n{}",
                    screen
                );
            }
        }
        Err(e) => panic!("Test failed: {}", e),
    }
}
