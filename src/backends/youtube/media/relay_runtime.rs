use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
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
    PreparedMedia, RelayPlanner, RelayPlayStrategy, RelayRangeError, RelaySessionId,
    RelaySessionSpec, RelaySessionState, UpstreamReadPlan,
};
use crate::backends::youtube::audio::{MpvInput, cache::AudioCache};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(60 * 10);

#[derive(Debug)]
struct RelaySessionRecord {
    spec: RelaySessionSpec,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct RelayRequestContext {
    session_id: String,
    track_id: String,
    peer_addr: Option<SocketAddr>,
    client_range: Option<String>,
    strategy: &'static str,
    upstream_host: String,
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
    pub fn start_with_cache(audio_cache: Arc<AudioCache>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind relay listener")?;
        let listen_addr = listener.local_addr().context("read relay listener addr")?;

        let running = Arc::new(AtomicBool::new(true));
        let sessions = Arc::new(DashMap::new());
        let http_client = Arc::new(
            reqwest::blocking::Client::builder()
                // Force IPv4: YouTube stream URLs are IP-locked to the address
                // used during extraction (ytx/yt-dlp). Without this, reqwest may
                // connect via IPv6, causing an IP mismatch → 403 Forbidden.
                // This matches the prefix download client in AudioCache.
                .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .context("build relay http client")?,
        );

        let running_worker = Arc::clone(&running);
        let sessions_worker = Arc::clone(&sessions);
        let client_worker = Arc::clone(&http_client);
        let cache_worker = Arc::clone(&audio_cache);

        let worker = thread::spawn(move || {
            while running_worker.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if !running_worker.load(Ordering::SeqCst) {
                            break;
                        }

                        prune_expired_sessions(&sessions_worker);
                        let sessions_conn = Arc::clone(&sessions_worker);
                        let client_conn = Arc::clone(&client_worker);
                        let cache_conn = Arc::clone(&cache_worker);
                        thread::spawn(move || {
                            if let Err(err) = handle_connection(
                                stream,
                                &sessions_conn,
                                &client_conn,
                                cache_conn.as_ref(),
                            ) {
                                log::warn!("relay connection failed: {err}");
                            }
                        });
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

    #[cfg(test)]
    pub fn start() -> Result<Self> {
        let temp_dir = tempfile::TempDir::new().context("create relay runtime cache tempdir")?;
        let cache = Arc::new(AudioCache::new(crate::backends::youtube::audio::CacheConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            prefix_size: 204_800,
            max_cache_size: 209_715_200,
        })?);
        let runtime = Self::start_with_cache(cache);
        std::mem::forget(temp_dir);
        runtime
    }

    pub fn register_session(&self, track_id: &str, prepared: &PreparedMedia) -> Result<MpvInput> {
        let mut spec = RelaySessionSpec::try_from_prepared(track_id.to_string(), prepared)
            .map_err(|e| anyhow!("relay contract violation: {e:?}"))?;
        spec.state = RelaySessionState::AwaitingRequest;

        let counter = self.session_counter.fetch_add(1, Ordering::Relaxed);
        let raw_id = format!("{}-{counter}", sanitize_for_session_id(track_id));
        let session_id =
            RelaySessionId::new(raw_id).map_err(|e| anyhow!("invalid session id: {e:?}"))?;

        let endpoint = spec.player_endpoint(self.listen_addr, session_id.clone());
        self.sessions.insert(
            session_id.as_str().to_string(),
            RelaySessionRecord { spec, expires_at: Instant::now() + SESSION_TTL },
        );

        Ok(MpvInput::new(endpoint.url()))
    }
}

impl Drop for RelayRuntime {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = TcpStream::connect(self.listen_addr);
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

    if out.is_empty() { "track".to_string() } else { out }
}

fn handle_connection(
    stream: TcpStream,
    sessions: &DashMap<String, RelaySessionRecord>,
    http_client: &reqwest::blocking::Client,
    audio_cache: &AudioCache,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).context("set relay read timeout")?;
    stream.set_write_timeout(Some(Duration::from_secs(30))).context("set relay write timeout")?;

    let mut reader = BufReader::new(stream.try_clone().context("clone relay stream")?);
    let request = parse_request(&mut reader)?;
    let mut writer = stream;
    let peer_addr = writer.peer_addr().ok();

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
        write_plain_response(
            &mut writer,
            status,
            reason_phrase(status),
            b"relay session terminated",
        )?;
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

    let strategy = RelayPlanner::from_session(&spec);
    let context = RelayRequestContext {
        session_id: session_key.clone(),
        track_id: spec.track_id.clone(),
        peer_addr,
        client_range: range_header.map(ToOwned::to_owned),
        strategy: relay_strategy_label(&strategy),
        upstream_host: upstream_host(&spec.upstream.url),
    };

    log::info!(
        "[RELAY] request start: session={} track={} peer={:?} client_range={:?} strategy={} staged={:?} upstream={:?} host={}",
        context.session_id,
        context.track_id,
        context.peer_addr,
        context.client_range,
        context.strategy,
        plan.staged,
        plan.upstream,
        context.upstream_host,
    );

    if let Err(err) =
        stream_strategy_response(&mut writer, &strategy, &spec, &plan, http_client, &context)
    {
        if let Some(mut entry) = sessions.get_mut(&session_key) {
            entry.spec.state = RelaySessionState::Failed;
        }
        return Err(err);
    }

    promote_tee_prefix_to_cache(audio_cache, &strategy, &spec)?;

    if let Some(mut entry) = sessions.get_mut(&session_key) {
        entry.spec.state = RelaySessionState::AwaitingRequest;
        entry.expires_at = Instant::now() + SESSION_TTL;
    }

    Ok(())
}

fn relay_strategy_label(strategy: &RelayPlayStrategy) -> &'static str {
    match strategy {
        RelayPlayStrategy::CacheHitRelay { .. } => "cache-hit",
        RelayPlayStrategy::TeeMissRelay { .. } => "tee-miss",
        RelayPlayStrategy::DirectFallback { .. } => "direct-fallback",
    }
}

fn upstream_host(upstream_url: &str) -> String {
    reqwest::Url::parse(upstream_url)
        .ok()
        .and_then(|url| url.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "<unknown>".to_string())
}

fn promote_tee_prefix_to_cache(
    audio_cache: &AudioCache,
    strategy: &RelayPlayStrategy,
    spec: &RelaySessionSpec,
) -> Result<()> {
    let RelayPlayStrategy::TeeMissRelay { track_id, prefix_target, .. } = strategy else {
        return Ok(());
    };

    let prefix_bytes = std::fs::metadata(&prefix_target.path)
        .with_context(|| format!("read tee prefix metadata {}", prefix_target.path.display()))?
        .len();

    if prefix_bytes == 0 {
        return Err(anyhow!("tee prefix promotion produced empty cache file for {}", track_id));
    }

    audio_cache.register_prefix(
        track_id,
        prefix_target.path.clone(),
        prefix_bytes,
        spec.upstream.content_length,
    );
    audio_cache.evict_lru()?;
    Ok(())
}

fn write_planned_response_headers(
    writer: &mut TcpStream,
    plan: &super::RelayResponsePlan,
    content_length: u64,
) -> Result<()> {
    let status = plan.status_code();
    let reason = if status == 206 { "Partial Content" } else { "OK" };
    write!(writer, "HTTP/1.1 {status} {reason}\r\n")?;
    write!(writer, "Accept-Ranges: bytes\r\n")?;
    write!(writer, "Content-Length: {}\r\n", plan.response.len())?;
    write!(writer, "Connection: close\r\n")?;
    if status == 206 {
        write!(
            writer,
            "Content-Range: {}\r\n",
            plan.response.to_content_range_value(content_length)
        )?;
    }
    write!(writer, "\r\n")?;

    Ok(())
}

fn stream_strategy_response(
    writer: &mut TcpStream,
    strategy: &RelayPlayStrategy,
    spec: &RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    http_client: &reqwest::blocking::Client,
    context: &RelayRequestContext,
) -> Result<()> {
    write_planned_response_headers(writer, plan, spec.upstream.content_length)?;

    match strategy {
        RelayPlayStrategy::TeeMissRelay { prefix_target, .. } => {
            stream_tee_response(writer, http_client, spec, prefix_target, context)?;
        }
        RelayPlayStrategy::CacheHitRelay { .. } => {
            stream_cache_hit_response(writer, http_client, strategy, spec, plan, context)?;
        }
        RelayPlayStrategy::DirectFallback { .. } => {
            return Err(anyhow!("direct fallback must be handled outside relay runtime"));
        }
    }

    writer.flush()?;
    Ok(())
}

fn stream_tee_response(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    spec: &RelaySessionSpec,
    prefix_target: &super::RelayTeePrefix,
    context: &RelayRequestContext,
) -> Result<()> {
    stream_tee_upstream(
        writer,
        http_client,
        &spec.upstream.url,
        spec.upstream.content_length,
        prefix_target,
        context,
    )?;
    Ok(())
}

fn stream_cache_hit_response(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    strategy: &RelayPlayStrategy,
    spec: &RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    context: &RelayRequestContext,
) -> Result<()> {
    if let Some(staged) = plan.staged {
        stream_staged_segment(writer, &spec.staged.path, staged.start, staged.len())?;
    }
    if let Some(upstream) = plan.upstream {
        let continuation_plans = strategy.continuation_plans(upstream.start, upstream.end);
        stream_upstream_with_recovery_plans(
            writer,
            http_client,
            &spec.upstream.url,
            continuation_plans.as_slice(),
            context,
        )?;
    }

    Ok(())
}

fn stream_staged_segment(writer: &mut TcpStream, path: &Path, start: u64, len: u64) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open staged artifact {}", path.display()))?;
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("seek staged artifact {}", path.display()))?;
    copy_exact(&mut file, writer, len)
}

/// Tee mode: opens ONE upstream connection from byte 0, writes first `tee.size`
/// bytes to `tee.path` on disk (prefix cache), and pipes everything to MPV.
fn stream_tee_upstream(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    content_length: u64,
    tee: &super::RelayTeePrefix,
    context: &RelayRequestContext,
) -> Result<()> {
    log::info!(
        "[RELAY] tee mode: session={} track={} host={} peer={:?} content_length={} streaming from byte 0, saving first {} bytes to {}",
        context.session_id,
        context.track_id,
        context.upstream_host,
        context.peer_addr,
        content_length,
        tee.size,
        tee.path.display()
    );

    let mut response = http_client
        .get(upstream_url)
        .header(reqwest::header::RANGE, "bytes=0-")
        .send()
        .with_context(|| format!("tee upstream request to {upstream_url}"))?;

    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(anyhow!(
            "tee upstream returned {status}, expected 200/206 for full-stream request"
        ));
    }

    // Create parent dirs for prefix file
    if let Some(parent) = tee.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create prefix cache dir {}", parent.display()))?;
    }

    let mut prefix_file = std::fs::File::create(&tee.path)
        .with_context(|| format!("create prefix file {}", tee.path.display()))?;

    let mut buf = [0u8; 32768];
    let mut total_written: u64 = 0;
    let mut prefix_written: u64 = 0;
    let tee_size = tee.size;

    loop {
        let n = response.read(&mut buf).with_context(|| "read from tee upstream")?;
        if n == 0 {
            break;
        }

        // Write to prefix file if we haven't filled it yet
        if prefix_written < tee_size {
            let to_cache = ((tee_size - prefix_written) as usize).min(n);
            prefix_file.write_all(&buf[..to_cache]).with_context(|| "write to prefix file")?;
            prefix_written += to_cache as u64;

            if prefix_written >= tee_size {
                prefix_file.flush()?;
                log::info!(
                    "[RELAY] tee: session={} track={} prefix cache complete ({} bytes saved to {})",
                    context.session_id,
                    context.track_id,
                    prefix_written,
                    tee.path.display()
                );
            }
        }

        // Always write to MPV
        writer.write_all(&buf[..n]).with_context(|| "write to MPV downstream")?;
        total_written += n as u64;
    }

    log::info!(
        "[RELAY] tee complete: session={} track={} host={} total_bytes={} prefix_cached={} expected_content_length={}",
        context.session_id,
        context.track_id,
        context.upstream_host,
        total_written,
        prefix_written,
        content_length,
    );

    if total_written < content_length {
        log::warn!(
            "[RELAY] tee short-read: session={} track={} host={} streamed={} expected={} remaining={}",
            context.session_id,
            context.track_id,
            context.upstream_host,
            total_written,
            content_length,
            content_length.saturating_sub(total_written),
        );
    }

    Ok(())
}

fn stream_upstream_with_recovery_plans(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    plans: &[UpstreamReadPlan],
    context: &RelayRequestContext,
) -> Result<()> {
    let mut last_err = None;

    for (attempt_idx, plan) in plans.iter().enumerate() {
        let started = Instant::now();
        match stream_upstream_segment(
            writer,
            http_client,
            upstream_url,
            *plan,
            context,
            attempt_idx + 1,
            plans.len(),
        ) {
            Ok(()) => return Ok(()),
            Err(err) => {
                log::warn!(
                    "[RELAY] upstream retry failed: session={} track={} host={} strategy={} attempt={}/{} plan={:?} elapsed_ms={} err={}",
                    context.session_id,
                    context.track_id,
                    context.upstream_host,
                    context.strategy,
                    attempt_idx + 1,
                    plans.len(),
                    plan,
                    started.elapsed().as_millis(),
                    err,
                );
                last_err = Some(err);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("no upstream read plans available")))
}

fn stream_upstream_segment(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    plan: UpstreamReadPlan,
    context: &RelayRequestContext,
    attempt: usize,
    attempt_total: usize,
) -> Result<()> {
    match plan {
        UpstreamReadPlan::QueryRange { start, end } => {
            let query_end = end.saturating_sub(1);
            log::info!(
                "[RELAY] upstream segment via query range: session={} track={} host={} attempt={}/{} bytes={}-{}",
                context.session_id,
                context.track_id,
                context.upstream_host,
                attempt,
                attempt_total,
                start,
                query_end,
            );
            stream_upstream_single_query_range(writer, http_client, upstream_url, start, end)
        }
        UpstreamReadPlan::AnchoredQueryRange { start, end } => {
            let query_end = end.saturating_sub(1);
            log::info!(
                "[RELAY] upstream segment via anchored query range: session={} track={} host={} attempt={}/{} bytes=0-{}, discard_to={}",
                context.session_id,
                context.track_id,
                context.upstream_host,
                attempt,
                attempt_total,
                query_end,
                start,
            );
            stream_upstream_query_range_from_zero(writer, http_client, upstream_url, start, end)
        }
        UpstreamReadPlan::ChunkedQueryRange { start, end } => {
            log::info!(
                "[RELAY] upstream segment via chunked query range: session={} track={} host={} attempt={}/{} bytes={}-{}",
                context.session_id,
                context.track_id,
                context.upstream_host,
                attempt,
                attempt_total,
                start,
                end.saturating_sub(1),
            );
            stream_upstream_chunked_range(writer, http_client, upstream_url, start, end)
        }
    }
}

fn stream_upstream_single_query_range(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    start: u64,
    end: u64,
) -> Result<()> {
    let query_end = end.saturating_sub(1);
    let ranged_url = format!("{upstream_url}&range={start}-{query_end}");
    let mut response = http_client
        .get(&ranged_url)
        .send()
        .with_context(|| format!("single query-range request to {ranged_url}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "single query-range request returned {status} for bytes={start}-{query_end}"
        ));
    }

    copy_exact(&mut response, writer, end.saturating_sub(start))
}

fn stream_upstream_query_range_from_zero(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    start: u64,
    end: u64,
) -> Result<()> {
    let query_end = end.saturating_sub(1);
    let ranged_url = format!("{upstream_url}&range=0-{query_end}");
    let mut response = http_client
        .get(&ranged_url)
        .send()
        .with_context(|| format!("anchored query-range request to {ranged_url}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "anchored query-range request returned {status} for bytes=0-{query_end}"
        ));
    }

    discard_exact(&mut response, start).with_context(|| {
        format!("discard first {start} bytes from anchored query-range response")
    })?;
    copy_exact(&mut response, writer, end.saturating_sub(start))
}

const RANGE_CHUNK_SIZE: u64 = 1_048_576; // 1MB chunks

fn stream_upstream_chunked_range(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    start: u64,
    end: u64,
) -> Result<()> {
    let total = end.saturating_sub(start);
    let mut pos = start;
    let mut chunk_num = 0u32;

    while pos < end {
        let chunk_end = (pos + RANGE_CHUNK_SIZE).min(end).saturating_sub(1);
        let ranged_url = format!("{upstream_url}&range={pos}-{chunk_end}&rn={chunk_num}");
        log::debug!("[RELAY] chunked &range={}-{} (chunk {})", pos, chunk_end, chunk_num);

        let mut response = http_client
            .get(&ranged_url)
            .send()
            .with_context(|| format!("chunked range request chunk {chunk_num}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!(
                "chunked &range= returned {status} at chunk {chunk_num} (pos={pos})"
            ));
        }

        let chunk_len = chunk_end - pos + 1;
        let mut remaining = chunk_len;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let to_read = (remaining as usize).min(buf.len());
            let n =
                response.read(&mut buf[..to_read]).with_context(|| "read from chunked upstream")?;
            if n == 0 {
                return Err(anyhow!(
                    "unexpected EOF from chunked upstream: chunk={} copied={} expected_chunk_bytes={} global_range={}-{}",
                    chunk_num,
                    chunk_len.saturating_sub(remaining),
                    chunk_len,
                    start,
                    end.saturating_sub(1),
                ));
            }
            writer.write_all(&buf[..n]).with_context(|| "write chunked data to MPV")?;
            remaining -= n as u64;
        }

        pos = chunk_end + 1;
        chunk_num += 1;
    }

    log::info!("[RELAY] chunked &range= complete: {} bytes in {} chunks", total, chunk_num);
    Ok(())
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
    let mut copied = 0u64;
    let mut buf = [0_u8; 64 * 1024];

    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = reader.read(&mut buf[..chunk]).context("read relay segment")?;
        if read == 0 {
            return Err(anyhow!(
                "unexpected EOF while streaming relay segment: copied {} of {} bytes (remaining {})",
                copied,
                len,
                remaining,
            ));
        }

        writer.write_all(&buf[..read]).context("write relay segment to client")?;
        let read = u64::try_from(read).unwrap_or(0);
        copied += read;
        remaining -= read;
    }

    Ok(())
}

fn discard_exact(reader: &mut impl Read, len: u64) -> Result<()> {
    let mut remaining = len;
    let mut discarded = 0u64;
    let mut buf = [0_u8; 64 * 1024];

    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = reader.read(&mut buf[..chunk]).context("discard relay prefix bytes")?;
        if read == 0 {
            return Err(anyhow!(
                "unexpected EOF while discarding relay prefix bytes: discarded {} of {} bytes (remaining {})",
                discarded,
                len,
                remaining,
            ));
        }
        let read = u64::try_from(read).unwrap_or(0);
        discarded += read;
        remaining -= read;
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
        self.headers.get(&name.to_ascii_lowercase()).map(std::string::String::as_str)
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
    let method = request_parts.next().ok_or_else(|| anyhow!("missing relay method"))?.to_string();
    let path = request_parts.next().ok_or_else(|| anyhow!("missing relay path"))?.to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Ok(ParsedRequest { method, path, headers })
}

fn extract_session_id_from_path(path: &str) -> Option<String> {
    let clean = path.split('?').next().unwrap_or(path);
    let mut parts = clean.split('/');

    match (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) {
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
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener, TcpStream},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use tempfile::TempDir;

    use super::{
        RANGE_CHUNK_SIZE, RelayRequestContext, RelayRuntime, copy_exact, discard_exact,
        extract_session_id_from_path, promote_tee_prefix_to_cache, stream_upstream_segment,
        stream_upstream_with_recovery_plans,
    };
    use crate::backends::youtube::{
        audio::{CacheConfig, cache::AudioCache},
        media::{
            RelayByteRange, RelayPlayStrategy, RelaySessionSpec, RelaySessionState,
            RelayStagedArtifact, RelayTeePrefix, RelayTransportContract, RelayUpstreamStream,
            UpstreamReadPlan,
        },
    };

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind pair listener");
        let addr = listener.local_addr().expect("pair local addr");
        let client = TcpStream::connect(addr).expect("connect pair");
        let (server, _) = listener.accept().expect("accept pair");
        (server, client)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).expect("read request");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(buf).expect("request utf8")
    }

    fn write_http_response(stream: &mut TcpStream, status_line: &str, body: &[u8]) {
        let headers = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).expect("write response headers");
        stream.write_all(body).expect("write response body");
    }

    fn parse_query_range(request_line: &str) -> Option<(u64, u64)> {
        let target = request_line.split_whitespace().nth(1)?;
        let range_idx = target.find("&range=")?;
        let range_part = &target[range_idx + "&range=".len()..];
        let range_value = range_part.split('&').next()?;
        let (start, end) = range_value.split_once('-')?;
        let start = start.parse::<u64>().ok()?;
        let end = end.parse::<u64>().ok()?;
        Some((start, end))
    }

    fn test_request_context() -> RelayRequestContext {
        RelayRequestContext {
            session_id: "session-1".to_string(),
            track_id: "track-123".to_string(),
            peer_addr: None,
            client_range: Some("bytes=0-".to_string()),
            strategy: "cache-hit",
            upstream_host: "example.com".to_string(),
        }
    }

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

    #[test]
    fn relay_runtime_drop_unblocks_accept_loop() {
        let started = Instant::now();
        let runtime = RelayRuntime::start().expect("start relay runtime");
        drop(runtime);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn tee_completion_promotes_prefix_into_audio_cache() {
        let temp_dir = TempDir::new().expect("tempdir");
        let cache = AudioCache::new(CacheConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            prefix_size: 204_800,
            max_cache_size: 209_715_200,
        })
        .expect("cache");
        let prefix_path = temp_dir.path().join("track-123.webm");
        std::fs::write(&prefix_path, vec![1u8; 1024]).expect("write prefix");

        let strategy = RelayPlayStrategy::TeeMissRelay {
            track_id: "track-123".to_string(),
            stream_url: "https://example.com/upstream?clen=4096".to_string(),
            prefix_target: RelayTeePrefix { path: prefix_path.clone(), size: 1024 },
        };
        let spec = RelaySessionSpec {
            track_id: "track-123".to_string(),
            staged: RelayStagedArtifact {
                path: prefix_path.clone(),
                available: RelayByteRange { start: 0, end: 0 },
            },
            upstream: RelayUpstreamStream {
                url: "https://example.com/upstream?clen=4096".to_string(),
                content_length: 4096,
            },
            contract: RelayTransportContract::default(),
            state: RelaySessionState::AwaitingRequest,
            tee_prefix: Some(RelayTeePrefix { path: prefix_path.clone(), size: 1024 }),
        };

        promote_tee_prefix_to_cache(&cache, &strategy, &spec).expect("promote tee prefix");

        assert_eq!(
            cache.get_prefix_metadata("track-123"),
            Some((PathBuf::from(&prefix_path), 1024, 4096))
        );
    }

    #[test]
    fn copy_exact_reports_short_read_details() {
        let mut reader = std::io::Cursor::new(vec![1u8, 2u8, 3u8]);
        let mut writer = Vec::new();

        let err = copy_exact(&mut reader, &mut writer, 8).expect_err("expected short read");
        let msg = err.to_string();

        assert!(msg.contains("copied 3 of 8 bytes"), "unexpected error: {msg}");
        assert!(msg.contains("remaining 5"), "unexpected error: {msg}");
    }

    #[test]
    fn discard_exact_reports_short_read_details() {
        let mut reader = std::io::Cursor::new(vec![1u8, 2u8, 3u8]);

        let err = discard_exact(&mut reader, 8).expect_err("expected short discard");
        let msg = err.to_string();

        assert!(msg.contains("discarded 3 of 8 bytes"), "unexpected error: {msg}");
        assert!(msg.contains("remaining 5"), "unexpected error: {msg}");
    }

    #[test]
    fn single_plan_executor_does_not_fallback_when_query_range_fails() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_server = Arc::clone(&request_count);
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");
        upstream_listener.set_nonblocking(true).expect("set upstream listener nonblocking");

        let server = thread::spawn(move || {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                let (mut stream, _) = match upstream_listener.accept() {
                    Ok(conn) => conn,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                request_count_server.fetch_add(1, Ordering::SeqCst);
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, "403 Forbidden", b"");
                break;
            }
        });

        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");
        let (mut writer, _reader) = tcp_pair();

        let result = stream_upstream_segment(
            &mut writer,
            &client,
            &upstream_url,
            UpstreamReadPlan::QueryRange { start: 128, end: 256 },
            &test_request_context(),
            1,
            1,
        );

        assert!(result.is_err(), "single-plan executor should return transport error");
        assert_eq!(request_count.load(Ordering::SeqCst), 1);

        server.join().expect("join upstream server");
    }

    #[test]
    fn upstream_403_recovers_via_single_query_range_without_chunked_rn() {
        let rn_requests = Arc::new(AtomicUsize::new(0));
        let rn_requests_server = Arc::clone(&rn_requests);
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");
        upstream_listener.set_nonblocking(true).expect("set upstream listener nonblocking");

        let server = thread::spawn(move || {
            let started = Instant::now();
            let mut handled = 0usize;
            while started.elapsed() < Duration::from_secs(2) {
                let (mut stream, _) = match upstream_listener.accept() {
                    Ok(conn) => conn,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if handled >= 2 {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let request = read_http_request(&mut stream);
                let request_line = request.lines().next().unwrap_or_default().to_string();
                let has_range_header =
                    request.lines().any(|line| line.to_ascii_lowercase().starts_with("range:"));
                let has_rn = request_line.contains("&rn=");
                handled += 1;

                if has_rn {
                    rn_requests_server.fetch_add(1, Ordering::SeqCst);
                    write_http_response(&mut stream, "403 Forbidden", b"");
                    continue;
                }

                if has_range_header {
                    write_http_response(&mut stream, "403 Forbidden", b"");
                    continue;
                }

                if let Some((start, end)) = parse_query_range(&request_line) {
                    let len = (end - start + 1) as usize;
                    let body = vec![b'x'; len];
                    write_http_response(&mut stream, "200 OK", &body);
                    continue;
                }

                write_http_response(&mut stream, "400 Bad Request", b"");
            }
        });

        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");

        let (mut writer, mut reader) = tcp_pair();
        let start = 204_800u64;
        let end = start + RANGE_CHUNK_SIZE + 17;

        let result = stream_upstream_with_recovery_plans(
            &mut writer,
            &client,
            &upstream_url,
            &[
                UpstreamReadPlan::QueryRange { start, end },
                UpstreamReadPlan::AnchoredQueryRange { start, end },
                UpstreamReadPlan::ChunkedQueryRange { start, end },
            ],
            &test_request_context(),
        );
        assert!(result.is_ok(), "expected recovery path to succeed: {result:?}");

        writer.shutdown(Shutdown::Write).expect("shutdown writer");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).expect("read streamed bytes");
        assert_eq!(streamed.len(), (end - start) as usize);
        assert_eq!(
            rn_requests.load(Ordering::SeqCst),
            0,
            "recovery should avoid chunked rn requests"
        );

        server.join().expect("join upstream server");
    }

    #[test]
    fn high_offset_query_range_recovers_via_anchored_zero_range_without_rn() {
        let rn_requests = Arc::new(AtomicUsize::new(0));
        let rn_requests_server = Arc::clone(&rn_requests);
        let header_range_requests = Arc::new(AtomicUsize::new(0));
        let header_range_requests_server = Arc::clone(&header_range_requests);
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");
        upstream_listener.set_nonblocking(true).expect("set upstream listener nonblocking");

        let server = thread::spawn(move || {
            let started = Instant::now();
            let mut handled = 0usize;
            while started.elapsed() < Duration::from_secs(3) {
                let (mut stream, _) = match upstream_listener.accept() {
                    Ok(conn) => conn,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if handled >= 2 {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let request = read_http_request(&mut stream);
                let request_line = request.lines().next().unwrap_or_default().to_string();
                let has_range_header =
                    request.lines().any(|line| line.to_ascii_lowercase().starts_with("range:"));
                let has_rn = request_line.contains("&rn=");
                handled += 1;

                if has_rn {
                    rn_requests_server.fetch_add(1, Ordering::SeqCst);
                    write_http_response(&mut stream, "403 Forbidden", b"");
                    continue;
                }

                if has_range_header {
                    header_range_requests_server.fetch_add(1, Ordering::SeqCst);
                    write_http_response(&mut stream, "403 Forbidden", b"");
                    continue;
                }

                if let Some((start, end)) = parse_query_range(&request_line) {
                    if start > 0 {
                        write_http_response(&mut stream, "403 Forbidden", b"");
                        continue;
                    }
                    let len = (end + 1) as usize;
                    let body: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
                    write_http_response(&mut stream, "200 OK", &body);
                    continue;
                }

                write_http_response(&mut stream, "400 Bad Request", b"");
            }
        });

        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");

        let (mut writer, mut reader) = tcp_pair();
        let start = 1_253_376u64;
        let end = 2_301_952u64;

        let result = stream_upstream_with_recovery_plans(
            &mut writer,
            &client,
            &upstream_url,
            &[
                UpstreamReadPlan::QueryRange { start, end },
                UpstreamReadPlan::AnchoredQueryRange { start, end },
                UpstreamReadPlan::ChunkedQueryRange { start, end },
            ],
            &test_request_context(),
        );
        assert!(result.is_ok(), "expected anchored zero-range recovery to succeed: {result:?}");

        writer.shutdown(Shutdown::Write).expect("shutdown writer");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).expect("read streamed bytes");
        assert_eq!(streamed.len(), (end - start) as usize);
        assert_eq!(
            streamed.first().copied(),
            Some((start % 251) as u8),
            "anchored path must discard prefix bytes before streaming"
        );
        assert_eq!(
            streamed.last().copied(),
            Some(((end - 1) % 251) as u8),
            "anchored path must preserve end boundary"
        );
        assert_eq!(
            rn_requests.load(Ordering::SeqCst),
            0,
            "recovery should avoid chunked rn requests"
        );
        assert_eq!(
            header_range_requests.load(Ordering::SeqCst),
            0,
            "recovery should avoid header range requests in this path"
        );

        server.join().expect("join upstream server");
    }
}
