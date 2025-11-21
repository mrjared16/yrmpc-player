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
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
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

        // Construct the command list directly
        let cmd_vec: Vec<Value> = args.iter().map(|&s| Value::String(s.to_string())).collect();

        let cmd_obj = serde_json::json!({
            "command": cmd_vec,
            "request_id": request_id
        });

        let mut json_str =
            serde_json::to_string(&cmd_obj).context("Failed to serialize command")?;
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
        let val_str = match value {
            Value::String(s) => s,
            v => v.to_string(),
        };
        self.send_command(vec!["set_property", property, &val_str])?;
        Ok(())
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
}
