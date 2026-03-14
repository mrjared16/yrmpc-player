use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;

use super::{
    PreparedMedia,
    RelayRangeError,
    RelaySessionId,
    RelaySessionSpec,
    RelaySessionState,
};
use crate::backends::youtube::audio::MpvInput;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(60 * 10);

#[derive(Debug)]
struct RelaySessionRecord {
    spec: RelaySessionSpec,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct RelayRuntime {
    listen_addr: SocketAddr,
    running: Arc<AtomicBool>,
    sessions: Arc<DashMap<String, RelaySessionRecord>>,
    session_counter: AtomicU64,
    worker: Option<thread::JoinHandle<()>>,
}

impl RelayRuntime {
    pub fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind relay listener")?;
        listener
            .set_nonblocking(true)
            .context("set relay listener nonblocking")?;
        let listen_addr = listener.local_addr().context("read relay listener addr")?;

        let running = Arc::new(AtomicBool::new(true));
        let sessions = Arc::new(DashMap::new());
        let http_client = Arc::new(
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .context("build relay http client")?,
        );

        let running_worker = Arc::clone(&running);
        let sessions_worker = Arc::clone(&sessions);
        let client_worker = Arc::clone(&http_client);

        let worker = thread::spawn(move || {
            while running_worker.load(Ordering::SeqCst) {
                prune_expired_sessions(&sessions_worker);

                match listener.accept() {
                    Ok((stream, _)) => {
                        let sessions_conn = Arc::clone(&sessions_worker);
                        let client_conn = Arc::clone(&client_worker);
                        thread::spawn(move || {
                            if let Err(err) = handle_connection(stream, &sessions_conn, &client_conn) {
                                log::warn!("relay connection failed: {err}");
                            }
                        });
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => {
                        log::warn!("relay accept failed: {err}");
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });

        Ok(Self {
            listen_addr,
            running,
            sessions,
            session_counter: AtomicU64::new(1),
            worker: Some(worker),
        })
    }

    pub fn register_session(&self, track_id: &str, prepared: &PreparedMedia) -> Result<MpvInput> {
        let mut spec = RelaySessionSpec::try_from_prepared(track_id.to_string(), prepared)
            .map_err(|e| anyhow!("relay contract violation: {e:?}"))?;
        spec.state = RelaySessionState::AwaitingRequest;

        let counter = self.session_counter.fetch_add(1, Ordering::Relaxed);
        let raw_id = format!("{}-{counter}", sanitize_for_session_id(track_id));
        let session_id = RelaySessionId::new(raw_id).map_err(|e| anyhow!("invalid session id: {e:?}"))?;

        let endpoint = spec.player_endpoint(self.listen_addr, session_id.clone());
        self.sessions.insert(
            session_id.as_str().to_string(),
            RelaySessionRecord {
                spec,
                expires_at: Instant::now() + SESSION_TTL,
            },
        );

        Ok(MpvInput::new(endpoint.url()))
    }
}

impl Drop for RelayRuntime {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn sanitize_for_session_id(track_id: &str) -> String {
    let mut out = String::with_capacity(track_id.len());
    for ch in track_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }

    if out.is_empty() {
        "track".to_string()
    } else {
        out
    }
}

fn handle_connection(
    stream: TcpStream,
    sessions: &DashMap<String, RelaySessionRecord>,
    http_client: &reqwest::blocking::Client,
) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("set relay read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .context("set relay write timeout")?;

    let mut reader = BufReader::new(stream.try_clone().context("clone relay stream")?);
    let request = parse_request(&mut reader)?;
    let mut writer = stream;

    if request.method != "GET" {
        write_plain_response(&mut writer, 405, "Method Not Allowed", b"method not allowed")?;
        return Ok(());
    }

    let Some(session_key) = extract_session_id_from_path(&request.path) else {
        write_plain_response(&mut writer, 404, "Not Found", b"unknown path")?;
        return Ok(());
    };

    let Some(mut entry) = sessions.get_mut(&session_key) else {
        write_plain_response(&mut writer, 404, "Not Found", b"unknown relay session")?;
        return Ok(());
    };

    if Instant::now() > entry.expires_at {
        drop(entry);
        sessions.remove(&session_key);
        write_plain_response(&mut writer, 404, "Not Found", b"expired relay session")?;
        return Ok(());
    }

    if let Some(status) = entry.spec.state.terminal_http_status() {
        write_plain_response(&mut writer, status, reason_phrase(status), b"relay session terminated")?;
        return Ok(());
    }

    entry.expires_at = Instant::now() + SESSION_TTL;

    let range_header = request.header("range");
    let plan = match entry.spec.plan_response(range_header) {
        Ok(plan) => plan,
        Err(RelayRangeError::Unsatisfiable) => {
            write_range_unsatisfiable(&mut writer, entry.spec.upstream.content_length)?;
            return Ok(());
        }
        Err(_) => {
            write_plain_response(&mut writer, 416, "Range Not Satisfiable", b"invalid range")?;
            return Ok(());
        }
    };

    entry.spec.state = RelaySessionState::Streaming;
    let spec = entry.spec.clone();
    drop(entry);

    if let Err(err) = stream_planned_response(&mut writer, &spec, &plan, http_client) {
        if let Some(mut entry) = sessions.get_mut(&session_key) {
            entry.spec.state = RelaySessionState::Failed;
        }
        return Err(err);
    }

    if let Some(mut entry) = sessions.get_mut(&session_key) {
        entry.spec.state = RelaySessionState::AwaitingRequest;
        entry.expires_at = Instant::now() + SESSION_TTL;
    }

    Ok(())
}

fn stream_planned_response(
    writer: &mut TcpStream,
    spec: &RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    http_client: &reqwest::blocking::Client,
) -> Result<()> {
    let status = plan.status_code();
    let reason = if status == 206 {
        "Partial Content"
    } else {
        "OK"
    };
    write!(writer, "HTTP/1.1 {status} {reason}\r\n")?;
    write!(writer, "Accept-Ranges: bytes\r\n")?;
    write!(writer, "Content-Length: {}\r\n", plan.response.len())?;
    write!(writer, "Connection: close\r\n")?;
    if status == 206 {
        write!(
            writer,
            "Content-Range: {}\r\n",
            plan.response.to_content_range_value(plan.content_length)
        )?;
    }
    write!(writer, "\r\n")?;

    if let Some(staged) = plan.staged {
        stream_staged_segment(writer, &spec.staged.path, staged.start, staged.len())?;
    }

    if let Some(upstream) = plan.upstream {
        stream_upstream_segment(
            writer,
            http_client,
            &spec.upstream.url,
            upstream.start,
            upstream.end,
        )?;
    }

    writer.flush()?;
    Ok(())
}

fn stream_staged_segment(
    writer: &mut TcpStream,
    path: &Path,
    start: u64,
    len: u64,
) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open staged artifact {}", path.display()))?;
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("seek staged artifact {}", path.display()))?;
    copy_exact(&mut file, writer, len)
}

fn stream_upstream_segment(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    start: u64,
    end: u64,
) -> Result<()> {
    let range_header = format!("bytes={}-{}", start, end.saturating_sub(1));
    let mut response = http_client
        .get(upstream_url)
        .header(reqwest::header::RANGE, range_header)
        .send()
        .with_context(|| format!("request upstream segment from {upstream_url}"))?;

    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(anyhow!(
            "upstream returned {}, expected 206 for ranged relay request",
            response.status()
        ));
    }

    let Some(content_range) = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(anyhow!("upstream 206 response missing Content-Range"));
    };

    let expected_prefix = format!("bytes {start}-");
    if !content_range.starts_with(&expected_prefix) {
        return Err(anyhow!(
            "unexpected upstream Content-Range '{content_range}', expected prefix '{expected_prefix}'"
        ));
    }

    copy_exact(&mut response, writer, end.saturating_sub(start))
}

fn prune_expired_sessions(sessions: &DashMap<String, RelaySessionRecord>) {
    let now = Instant::now();
    sessions.retain(|_, record| record.expires_at > now);
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        404 => "Not Found",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

fn copy_exact(reader: &mut impl Read, writer: &mut impl Write, len: u64) -> Result<()> {
    let mut remaining = len;
    let mut buf = [0_u8; 64 * 1024];

    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = reader.read(&mut buf[..chunk]).context("read relay segment")?;
        if read == 0 {
            return Err(anyhow!("unexpected EOF while streaming relay segment"));
        }

        writer
            .write_all(&buf[..read])
            .context("write relay segment to client")?;
        remaining -= u64::try_from(read).unwrap_or(0);
    }

    Ok(())
}

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl ParsedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(std::string::String::as_str)
    }
}

fn parse_request(reader: &mut impl BufRead) -> Result<ParsedRequest> {
    let mut header_bytes = Vec::with_capacity(1024);
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(anyhow!("unexpected EOF while reading relay request"));
        }

        header_bytes.extend_from_slice(line.as_bytes());
        if header_bytes.len() > MAX_HEADER_BYTES {
            return Err(anyhow!("relay request headers too large"));
        }

        if line == "\r\n" {
            break;
        }
    }

    let header_text = String::from_utf8(header_bytes).context("relay request headers not utf8")?;
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());

    let request_line = lines.next().ok_or_else(|| anyhow!("missing relay request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing relay method"))?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing relay path"))?
        .to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Ok(ParsedRequest {
        method,
        path,
        headers,
    })
}

fn extract_session_id_from_path(path: &str) -> Option<String> {
    let clean = path.split('?').next().unwrap_or(path);
    let mut parts = clean.split('/');

    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(""), Some("relay"), Some("sessions"), Some(id), Some("stream"), None) => {
            Some(id.to_string())
        }
        _ => None,
    }
}

fn write_plain_response(
    writer: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &[u8],
) -> Result<()> {
    write!(writer, "HTTP/1.1 {status} {reason}\r\n")?;
    write!(writer, "Content-Type: text/plain; charset=utf-8\r\n")?;
    write!(writer, "Content-Length: {}\r\n", body.len())?;
    write!(writer, "Connection: close\r\n\r\n")?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

fn write_range_unsatisfiable(writer: &mut TcpStream, content_length: u64) -> Result<()> {
    write!(writer, "HTTP/1.1 416 Range Not Satisfiable\r\n")?;
    write!(writer, "Content-Range: bytes */{content_length}\r\n")?;
    write!(writer, "Content-Length: 0\r\n")?;
    write!(writer, "Connection: close\r\n\r\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_session_id_from_path;

    #[test]
    fn extracts_session_id_from_valid_path() {
        assert_eq!(
            extract_session_id_from_path("/relay/sessions/abc123/stream"),
            Some("abc123".to_string())
        );
        assert_eq!(
            extract_session_id_from_path("/relay/sessions/abc123/stream?x=y"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn rejects_invalid_paths() {
        assert_eq!(extract_session_id_from_path("/relay/session/abc/stream"), None);
        assert_eq!(extract_session_id_from_path("/relay/sessions/abc"), None);
        assert_eq!(extract_session_id_from_path("/other"), None);
    }
}
