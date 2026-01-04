use std::{path::PathBuf, thread, time::Duration};

use anyhow::{Context, Result};
use crossbeam::channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::shared::events::AppEvent;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ScriptEvent {
    Key { code: String, modifiers: Option<String> },
    Sleep { ms: u64 },
    Comment { text: String },
}

pub(crate) fn run_script(path: PathBuf, tx: Sender<AppEvent>) -> Result<()> {
    let content = std::fs::read_to_string(&path).context("Failed to read script file")?;
    let events: Vec<ScriptEvent> =
        serde_json::from_str(&content).context("Failed to parse script file")?;

    thread::spawn(move || {
        log::info!("Starting headless script execution");
        // Give the app some time to initialize
        thread::sleep(Duration::from_millis(1000));

        for event in events {
            match event {
                ScriptEvent::Key { code, modifiers } => {
                    let code = match code.as_str() {
                        "Enter" => KeyCode::Enter,
                        "Esc" => KeyCode::Esc,
                        "Backspace" => KeyCode::Backspace,
                        "Left" => KeyCode::Left,
                        "Right" => KeyCode::Right,
                        "Up" => KeyCode::Up,
                        "Down" => KeyCode::Down,
                        "Tab" => KeyCode::Tab,
                        c if c.len() == 1 => KeyCode::Char(c.chars().next().unwrap()),
                        _ => {
                            log::warn!("Unknown key code in script: {}", code);
                            continue;
                        }
                    };

                    let modifiers = match modifiers.as_deref() {
                        Some("Ctrl") => KeyModifiers::CONTROL,
                        Some("Alt") => KeyModifiers::ALT,
                        Some("Shift") => KeyModifiers::SHIFT,
                        _ => KeyModifiers::NONE,
                    };

                    let key_event = KeyEvent::new(code, modifiers);
                    if let Err(e) = tx.send(AppEvent::UserKeyInput(key_event)) {
                        log::error!("Failed to send script event: {}", e);
                        break;
                    }
                    // Small delay to simulate typing
                    thread::sleep(Duration::from_millis(100));
                }
                ScriptEvent::Sleep { ms } => {
                    thread::sleep(Duration::from_millis(ms));
                }
                ScriptEvent::Comment { text } => {
                    log::info!("Script comment: {}", text);
                }
            }
        }
        log::info!("Headless script execution finished");
    });

    Ok(())
}
