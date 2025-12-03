//! Proof-of-concept: Can expectrl interact with a crossterm-based TUI app?

use std::time::Duration;
use expectrl::{spawn, Expect};  // Expect trait needed for send/expect methods

/// Test 1: Can we spawn rmpc and see ANY output?
#[test]
#[ignore] // Run with: cargo test --test expectrl_test -- --ignored --nocapture
fn test_spawn_and_see_output() {
    
    let rmpc_bin = std::env::current_dir()
        .unwrap()
        .join("target/release/rmpc");
    
    let config_path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("config/rmpc.ron");
    
    println!("Binary: {:?}", rmpc_bin);
    println!("Config: {:?}", config_path);
    
    if !rmpc_bin.exists() {
        panic!("rmpc binary not found at {:?}. Run 'cargo build --release' first.", rmpc_bin);
    }
    
    let cmd = format!("{} --config {}", rmpc_bin.display(), config_path.display());
    println!("Command: {}", cmd);
    
    let mut p = spawn(&cmd).expect("Failed to spawn rmpc");
    
    // Set a reasonable timeout
    p.set_expect_timeout(Some(Duration::from_secs(5)));
    
    // Try to read ANY output for 2 seconds
    println!("Waiting for any output...");
    
    // Read raw bytes to see what we get
    let mut buffer = vec![0u8; 4096];
    match p.try_read(&mut buffer) {
        Ok(n) => {
            println!("Got {} bytes of output", n);
            println!("Raw bytes: {:?}", &buffer[..n.min(200)]);
            // Try to interpret as string (will have escape codes)
            let output = String::from_utf8_lossy(&buffer[..n]);
            println!("As string (first 500 chars): {}", &output[..output.len().min(500)]);
        }
        Err(e) => {
            println!("Error reading: {:?}", e);
        }
    }
    
    // Try to send 'q' to quit
    println!("Sending 'q' to quit...");
    let _ = p.send("q");
    
    std::thread::sleep(Duration::from_millis(500));
    
    // Check if process exited
    println!("Checking process status...");
}

/// Test 2: Test with a simple cat command to verify PTY input works
#[test]
#[ignore]
fn test_pty_input_with_cat() {
    // Test with cat first to verify PTY input works at all
    println!("Testing PTY input with 'cat' command...");
    
    let mut p = spawn("cat").expect("Failed to spawn cat");
    p.set_expect_timeout(Some(Duration::from_secs(2)));
    
    // Send some text
    p.send("hello\n").expect("Failed to send");
    
    // cat should echo it back
    match p.expect("hello") {
        Ok(_) => println!("SUCCESS! cat echoed 'hello' - PTY input works!"),
        Err(e) => println!("FAILED: cat didn't echo: {:?}", e),
    }
    
    // Send Ctrl+D to exit
    p.send("\x04").ok();
}

/// Test 3: Test rmpc through bash (shell handles terminal setup)
#[test]
#[ignore] 
fn test_rmpc_through_bash() {
    use std::process::Command;
    
    let rmpc_bin = std::env::current_dir()
        .unwrap()
        .join("target/release/rmpc");
    let config_path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("config/rmpc.ron");
    
    println!("Testing rmpc through bash shell...");
    
    // Spawn bash, then run rmpc from within bash
    // This ensures proper terminal setup
    let mut p = spawn("bash --norc --noprofile").expect("Failed to spawn bash");
    p.set_expect_timeout(Some(Duration::from_secs(10)));
    
    std::thread::sleep(Duration::from_millis(500));
    
    // Start rmpc from bash
    let cmd = format!("{} --config {}\n", rmpc_bin.display(), config_path.display());
    println!("Sending command: {}", cmd.trim());
    p.send(&cmd).expect("Failed to send rmpc command");
    
    // Wait for UI
    std::thread::sleep(Duration::from_secs(2));
    
    println!("Looking for Search...");
    match p.expect("Search") {
        Ok(_) => println!("  Found 'Search' - UI rendered!"),
        Err(e) => {
            println!("  Failed to find Search: {:?}", e);
            return;
        }
    }
    
    // Now try to send input
    println!("Sending '/' to enter search mode...");
    p.send("/").expect("Failed to send /");
    std::thread::sleep(Duration::from_millis(500));
    
    println!("Typing 'test'...");
    p.send("test").expect("Failed to send test");
    std::thread::sleep(Duration::from_millis(500));
    
    // Check if input appeared
    println!("Checking if 'test' appeared...");
    match p.expect("test") {
        Ok(_) => println!("SUCCESS! Input reached rmpc through bash!"),
        Err(_) => println!("FAILED - input still not reaching rmpc"),
    }
    
    // Quit
    p.send("\x1b").ok(); // ESC
    std::thread::sleep(Duration::from_millis(200));
    p.send("q").ok();
    std::thread::sleep(Duration::from_millis(500));
    p.send("exit\n").ok();
}

/// Test 3: Full interaction - search for something
#[test]
#[ignore]
fn test_full_interaction() {
    use std::process::Command;
    
    let rmpc_bin = std::env::current_dir()
        .unwrap()
        .join("target/release/rmpc");
    
    let config_path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("config/rmpc.ron");
    
    if !rmpc_bin.exists() {
        panic!("rmpc binary not found");
    }
    
    // Use log file to capture what happens
    let log_file = "/tmp/rmpc-expectrl-test.log";
    
    // Use Command to properly set environment variables
    let mut cmd = Command::new(&rmpc_bin);
    cmd.arg("--config").arg(&config_path);
    cmd.env("RMPC_LOG_FILE", log_file);
    cmd.env("RUST_LOG", "debug");
    
    println!("Spawning: {:?}", cmd);
    
    let mut p = expectrl::session::Session::spawn(cmd).expect("Failed to spawn rmpc");
    p.set_expect_timeout(Some(Duration::from_secs(10)));
    
    // Wait for UI to render
    println!("Step 1: Waiting for Search tab...");
    match p.expect("Search") {
        Ok(_) => println!("  Found 'Search'"),
        Err(e) => {
            println!("  Failed: {:?}", e);
            return;
        }
    }
    
    // Wait a bit more for app to fully initialize  
    std::thread::sleep(Duration::from_secs(1));
    
    // Try pressing '/' to enter search mode (EnterSearch keybind)
    println!("Step 2: Sending '/' to enter search mode...");
    p.send("/").expect("Failed to send '/'");
    std::thread::sleep(Duration::from_millis(500));
    
    println!("Step 3: Typing 'lofi'...");
    // Send characters one by one with small delay
    for c in "lofi".chars() {
        p.send(&c.to_string()).expect("Failed to send char");
        std::thread::sleep(Duration::from_millis(100));
    }
    
    std::thread::sleep(Duration::from_millis(500));
    
    // Check if text appeared in the search box
    println!("  Checking if 'lofi' is visible...");
    match p.expect("lofi") {
        Ok(_) => println!("  SUCCESS! 'lofi' found in output - keystrokes ARE working!"),
        Err(_) => println!("  'lofi' NOT found - keystrokes may not be reaching the app"),
    }
    
    println!("Step 4: Pressing Enter to search...");
    p.send("\r").expect("Failed to send Enter");
    
    // Wait longer for search to complete (network request)
    println!("  Waiting for search results (5 sec)...");
    std::thread::sleep(Duration::from_secs(5));
    
    // Try to capture what's on screen now
    println!("Step 5: Checking screen content...");
    let mut buffer = vec![0u8; 8192];
    match p.try_read(&mut buffer) {
        Ok(n) if n > 0 => {
            let output = String::from_utf8_lossy(&buffer[..n]);
            println!("  Screen has {} bytes", n);
            // Look for any song-like content
            if output.contains("lofi") || output.contains("Lofi") || output.contains("results") {
                println!("  GOOD: Found search-related content!");
            } else {
                println!("  Screen content (sample): {}", &output[..output.len().min(500)]);
            }
        }
        _ => println!("  Could not read screen"),
    }
    
    println!("Step 6: Quitting...");
    // Press Escape first to exit any mode, then q
    p.send("\x1b").expect("Failed to send Escape");  // ESC
    std::thread::sleep(Duration::from_millis(300));
    p.send("q").expect("Failed to send q");
    
    std::thread::sleep(Duration::from_secs(1));
    
    // Check the log file
    println!("\n--- Log file contents ---");
    if let Ok(log) = std::fs::read_to_string(log_file) {
        // Print last 2000 chars
        let start = log.len().saturating_sub(2000);
        println!("{}", &log[start..]);
    } else {
        println!("Could not read log file");
    }
}
