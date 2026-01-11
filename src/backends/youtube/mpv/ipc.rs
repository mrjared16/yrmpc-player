use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
struct MpvCommand {
    command: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MpvResponse {
    pub error: Option<String>,
    pub data: Option<Value>,
    pub request_id: Option<u64>,
    #[allow(dead_code)]
    pub event: Option<String>,
    /// For property-change events, the property name
    pub name: Option<String>,
    /// For end-file events, the reason (eof, error, stop, etc.)
    pub reason: Option<String>,
    /// For end-file error events, the actual error string (e.g., "loading
    /// failed")
    #[serde(rename = "file-error")]
    pub file_error: Option<String>,
}

/// Parsed MPV event for easier handling
#[derive(Debug, Clone)]
pub enum MpvEvent {
    /// Track changed (playlist-pos property changed)
    TrackChanged { position: i64 },
    /// Pause state changed
    PauseChanged { paused: bool },
    /// File ended (eof = natural end, error = playback failed)
    EndFile { reason: String, file_error: Option<String> },
    /// Idle state changed
    IdleChanged { idle: bool },
    /// Time remaining in current track (seconds)
    TimeRemaining { seconds: f64 },
    /// Some other event we don't care about
    Other(String),
}

#[derive(Debug)]
pub struct MpvIpc {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    request_id: u64,
}

impl MpvIpc {
    pub fn connect<P: AsRef<Path>>(path: P) -> Result<Self> {
        let stream = UnixStream::connect(path).context("Failed to connect to MPV socket")?;
        // Set longer read timeout to reduce CPU spinning in event loops
        // Events are still processed immediately when they arrive
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .context("Failed to set read timeout")?;

        Ok(Self {
            reader: BufReader::new(stream.try_clone().context("Failed to clone stream")?),
            writer: stream,
            request_id: 1,
        })
    }

    pub fn try_clone_stream(&self) -> Result<UnixStream> {
        self.writer.try_clone().context("Failed to clone MPV stream")
    }

    pub fn send_command(&mut self, args: Vec<&str>) -> Result<Value> {
        let request_id = self.request_id;
        self.request_id += 1;

        let cmd = MpvCommand {
            command: args.iter().map(|&s| Value::String(s.to_string())).collect(),
            request_id: Some(request_id),
        };

        let mut json_str = serde_json::to_string(&cmd).context("Failed to serialize command")?;
        json_str.push('\n');

        self.writer.write_all(json_str.as_bytes()).context("Failed to write to MPV socket")?;
        self.writer.flush().context("Failed to flush MPV socket")?;

        // Read response
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err(anyhow::anyhow!("MPV socket closed")),
                Ok(_) => {
                    // Try to parse as response
                    if let Ok(resp) = serde_json::from_str::<MpvResponse>(&line) {
                        if let Some(id) = resp.request_id {
                            if id == request_id {
                                if let Some(err) = resp.error {
                                    if err != "success" {
                                        return Err(anyhow::anyhow!("MPV error: {}", err));
                                    }
                                }
                                return Ok(resp.data.unwrap_or(Value::Null));
                            }
                        }
                    }
                    // Ignore events or other responses for now
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    return Err(anyhow::Error::new(e).context("Failed to read from MPV socket"));
                }
            }
        }
    }

    pub fn get_property(&mut self, property: &str) -> Result<Value> {
        self.send_command(vec!["get_property", property])
    }

    pub fn set_property(&mut self, property: &str, value: Value) -> Result<()> {
        // Use set_property_native which accepts the JSON value directly
        let request_id = self.request_id;
        self.request_id += 1;

        let cmd = MpvCommand {
            command: vec![
                Value::String("set_property".to_string()),
                Value::String(property.to_string()),
                value,
            ],
            request_id: Some(request_id),
        };

        let mut json_str = serde_json::to_string(&cmd).context("Failed to serialize command")?;
        json_str.push('\n');

        self.writer.write_all(json_str.as_bytes()).context("Failed to write to MPV socket")?;
        self.writer.flush().context("Failed to flush MPV socket")?;

        // Read response
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err(anyhow::anyhow!("MPV socket closed")),
                Ok(_) => {
                    if let Ok(resp) = serde_json::from_str::<MpvResponse>(&line) {
                        if let Some(id) = resp.request_id {
                            if id == request_id {
                                if let Some(err) = resp.error {
                                    if err != "success" {
                                        return Err(anyhow::anyhow!("MPV error: {}", err));
                                    }
                                }
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    return Err(anyhow::Error::new(e).context("Failed to read from MPV socket"));
                }
            }
        }
    }

    pub fn receive_message(&mut self) -> Result<MpvResponse> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(anyhow::anyhow!("MPV socket closed"));
        }
        let resp: MpvResponse = serde_json::from_str(&line)
            .context(format!("Failed to parse MPV response: {}", line))?;
        Ok(resp)
    }

    /// Observe a property - MPV will send events when it changes
    /// Returns the observer ID for later unobserving
    pub fn observe_property(&mut self, observer_id: u64, property: &str) -> Result<()> {
        let cmd = MpvCommand {
            command: vec![
                Value::String("observe_property".to_string()),
                Value::Number(observer_id.into()),
                Value::String(property.to_string()),
            ],
            request_id: None,
        };

        let mut json_str = serde_json::to_string(&cmd).context("Failed to serialize command")?;
        json_str.push('\n');

        self.writer.write_all(json_str.as_bytes()).context("Failed to write to MPV socket")?;
        self.writer.flush().context("Failed to flush MPV socket")?;

        // Read and discard response (we just need success)
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err(anyhow::anyhow!("MPV socket closed")),
                Ok(_) => {
                    if let Ok(resp) = serde_json::from_str::<MpvResponse>(&line) {
                        // Check if this is the response to our command (has error field)
                        if resp.error.is_some() {
                            if resp.error.as_deref() == Some("success") {
                                return Ok(());
                            } else {
                                return Err(anyhow::anyhow!("MPV error: {:?}", resp.error));
                            }
                        }
                    }
                    // Continue reading until we get a response
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    return Err(anyhow::Error::new(e).context("Failed to read from MPV socket"));
                }
            }
        }
    }

    /// Read next event from MPV (blocking with timeout)
    /// Returns parsed MpvEvent, or None on timeout
    ///
    /// The caller should handle None by sleeping or checking running flags
    /// to avoid busy-spinning.
    pub fn read_event(&mut self) -> Result<Option<MpvEvent>> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err(anyhow::anyhow!("MPV socket closed")),
                Ok(_) => {
                    if let Ok(resp) = serde_json::from_str::<MpvResponse>(&line) {
                        // Skip command responses (have request_id)
                        if resp.request_id.is_some() {
                            continue;
                        }

                        // Parse event
                        if let Some(event) = &resp.event {
                            return Ok(Some(Self::parse_event(event, &resp)));
                        }
                    }
                    // Continue reading for actual events
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        // Timeout - return None so caller can sleep/check running flag
                        // This prevents busy-spinning when no events are available
                        return Ok(None);
                    }
                    return Err(anyhow::Error::new(e).context("Failed to read from MPV socket"));
                }
            }
        }
    }

    /// Parse MPV event from response
    fn parse_event(event_name: &str, resp: &MpvResponse) -> MpvEvent {
        match event_name {
            "property-change" => {
                if let Some(name) = &resp.name {
                    match name.as_str() {
                        "playlist-pos" => {
                            let pos = resp.data.as_ref().and_then(|v| v.as_i64()).unwrap_or(-1);
                            MpvEvent::TrackChanged { position: pos }
                        }
                        "pause" => {
                            let paused =
                                resp.data.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                            MpvEvent::PauseChanged { paused }
                        }
                        "idle-active" => {
                            let idle =
                                resp.data.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                            MpvEvent::IdleChanged { idle }
                        }
                        "time-remaining" => {
                            let seconds =
                                resp.data.as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0);
                            MpvEvent::TimeRemaining { seconds }
                        }
                        _ => MpvEvent::Other(format!("property-change: {}", name)),
                    }
                } else {
                    MpvEvent::Other("property-change: unknown".to_string())
                }
            }
            "end-file" => {
                let reason = resp.reason.clone().unwrap_or_else(|| "unknown".to_string());
                let file_error = resp.file_error.clone();
                MpvEvent::EndFile { reason, file_error }
            }
            other => MpvEvent::Other(other.to_string()),
        }
    }
}
