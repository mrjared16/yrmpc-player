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
use crate::backends::youtube::url_resolver::UrlResolver;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(60 * 10);

#[derive(Debug)]
struct RelaySessionRecord {
    spec: RelaySessionSpec,
    expires_at: Instant,
    /// Whether a connection is currently active for this session.
    /// Prevents concurrent connections to the same session.
    active_connection: bool,
    recovery_attempts: u8,
    switched_extractor: bool,
}

#[derive(Debug, Clone)]
struct RelayRequestContext {
    session_id: String,
    track_id: String,
    peer_addr: Option<SocketAddr>,
    client_range: Option<String>,
    strategy: &'static str,
    upstream_host: String,
    upstream_url: String,
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
    pub fn start_with_cache(
        audio_cache: Arc<AudioCache>,
        url_resolver: Option<Arc<UrlResolver>>,
    ) -> Result<Self> {
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
        let url_resolver_worker = url_resolver.clone();

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
                        let url_resolver_conn = url_resolver_worker.clone();
                        thread::spawn(move || {
                            if let Err(err) = handle_connection(
                                stream,
                                &sessions_conn,
                                &client_conn,
                                cache_conn.as_ref(),
                                url_resolver_conn.as_deref(),
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
        let runtime = Self::start_with_cache(cache, None);
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
            RelaySessionRecord {
                spec,
                expires_at: Instant::now() + SESSION_TTL,
                active_connection: false,
                recovery_attempts: 0,
                switched_extractor: false,
            },
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
    url_resolver: Option<&UrlResolver>,
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

    // Reject concurrent connections to the same session
    if entry.active_connection {
        write_plain_response(&mut writer, 503, "Service Unavailable", b"session busy")?;
        return Ok(());
    }
    entry.active_connection = true;

    if let Some(status) = entry.spec.state.terminal_http_status() {
        entry.active_connection = false;
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
            entry.active_connection = false;
            write_range_unsatisfiable(&mut writer, entry.spec.upstream.content_length)?;
            return Ok(());
        }
        Err(_) => {
            entry.active_connection = false;
            write_plain_response(&mut writer, 416, "Range Not Satisfiable", b"invalid range")?;
            return Ok(());
        }
    };

    entry.spec.state = RelaySessionState::Streaming;
    let mut spec = entry.spec.clone();
    drop(entry);

    let strategy = RelayPlanner::from_session(&spec);
    let context = RelayRequestContext {
        session_id: session_key.clone(),
        track_id: spec.track_id.clone(),
        peer_addr,
        client_range: range_header.map(ToOwned::to_owned),
        strategy: relay_strategy_label(&strategy),
        upstream_host: upstream_host(&spec.upstream.url),
        upstream_url: spec.upstream.url.clone(),
    };

    log::info!(
        "[RELAY] request start: session={} track={} peer={:?} client_range={:?} strategy={} staged={:?} upstream={:?} host={} url={}",
        context.session_id,
        context.track_id,
        context.peer_addr,
        context.client_range,
        context.strategy,
        plan.staged,
        plan.upstream,
        context.upstream_host,
        context.upstream_url,
    );

    let stream_err = stream_strategy_response(
        &mut writer,
        &strategy,
        &mut spec,
        &plan,
        http_client,
        url_resolver,
        &context,
    );

    let failure_kind = stream_err.as_ref().err().map(classify_stream_error);
    let refreshed_url_in_request = spec.upstream.url != context.upstream_url;

    if let Some(mut entry) = sessions.get_mut(&session_key) {
        entry.active_connection = false;
        if refreshed_url_in_request {
            entry.spec.upstream.url = spec.upstream.url.clone();
        }
        match failure_kind {
            None => {
                entry.spec.state = RelaySessionState::AwaitingRequest;
                entry.expires_at = Instant::now() + SESSION_TTL;
            }
            Some(RelayStreamFailureKind::Retryable) | Some(RelayStreamFailureKind::ExpiredUrl) => {
                entry.spec.state = RelaySessionState::AwaitingRequest;
                entry.expires_at = Instant::now() + SESSION_TTL;

                if matches!(failure_kind, Some(RelayStreamFailureKind::Retryable)) {
                    log::info!(
                        "[RELAY] retryable error session={} track={} outcome=awaiting-request",
                        context.session_id,
                        context.track_id,
                    );
                }

                if matches!(failure_kind, Some(RelayStreamFailureKind::ExpiredUrl)) {
                    if refreshed_url_in_request && entry.recovery_attempts == 0 {
                        entry.recovery_attempts = 1;
                    }

                    let attempt = entry.recovery_attempts;
                    let recovery_result = if attempt == 0 {
                        entry.recovery_attempts = 1;
                        url_resolver.map(|resolver| resolver.refresh_url_forced(&spec.track_id))
                    } else if attempt == 1 && !entry.switched_extractor {
                        entry.recovery_attempts = 2;
                        entry.switched_extractor = true;
                        url_resolver
                            .map(|resolver| resolver.switch_and_extract_fresh(&spec.track_id))
                    } else {
                        None
                    };

                    match recovery_result {
                        Some(Ok(fresh_url)) => {
                            log::info!(
                                "[RELAY] 403 recovery session={} track={} step={} outcome=refreshed",
                                context.session_id,
                                context.track_id,
                                entry.recovery_attempts,
                            );
                            entry.spec.upstream.url = fresh_url;
                        }
                        Some(Err(refresh_err)) => {
                            log::info!(
                                "[RELAY] 403 recovery session={} track={} step={} outcome=failed err={}",
                                context.session_id,
                                context.track_id,
                                entry.recovery_attempts,
                                refresh_err,
                            );
                            entry.spec.state = RelaySessionState::Failed;
                        }
                        None if !refreshed_url_in_request => {
                            log::info!(
                                "[RELAY] 403 recovery session={} track={} step={} outcome=exhausted",
                                context.session_id,
                                context.track_id,
                                entry.recovery_attempts,
                            );
                            entry.spec.state = RelaySessionState::Failed;
                        }
                        None => {
                            log::info!(
                                "[RELAY] 403 recovery session={} track={} step={} outcome=already-refreshed",
                                context.session_id,
                                context.track_id,
                                entry.recovery_attempts,
                            );
                            entry.spec.state = RelaySessionState::Failed;
                        }
                    }
                }
            }
            Some(RelayStreamFailureKind::Terminal) => {
                entry.spec.state = RelaySessionState::Failed;
            }
        }
    }

    if let Err(err) = stream_err {
        return Err(err);
    }

    promote_tee_prefix_to_cache(audio_cache, &strategy, &spec)?;

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

    let prefix_bytes = match std::fs::metadata(&prefix_target.path) {
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("read tee prefix metadata {}", prefix_target.path.display())
            });
        }
    };

    if prefix_bytes < prefix_target.size {
        log::info!(
            "[RELAY] skip tee prefix promotion for incomplete prefix: track={} bytes={} target={} path={}",
            track_id,
            prefix_bytes,
            prefix_target.size,
            prefix_target.path.display(),
        );
        return Ok(());
    }

    if prefix_bytes > prefix_target.size {
        log::warn!(
            "[RELAY] tee prefix larger than target during promotion: track={} bytes={} target={} path={}",
            track_id,
            prefix_bytes,
            prefix_target.size,
            prefix_target.path.display(),
        );
    }

    audio_cache.register_prefix(
        track_id,
        prefix_target.path.clone(),
        prefix_bytes.min(prefix_target.size),
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
    spec: &mut RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    http_client: &reqwest::blocking::Client,
    url_resolver: Option<&UrlResolver>,
    context: &RelayRequestContext,
) -> Result<()> {
    write_planned_response_headers(writer, plan, spec.upstream.content_length)?;

    match strategy {
        RelayPlayStrategy::TeeMissRelay { prefix_target, .. } => {
            stream_tee_response(
                writer,
                http_client,
                strategy,
                spec,
                plan,
                prefix_target,
                url_resolver,
                context,
            )?;
        }
        RelayPlayStrategy::CacheHitRelay { .. } => {
            stream_cache_hit_response(
                writer,
                http_client,
                strategy,
                spec,
                plan,
                url_resolver,
                context,
            )?;
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
    strategy: &RelayPlayStrategy,
    spec: &mut RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    prefix_target: &super::RelayTeePrefix,
    url_resolver: Option<&UrlResolver>,
    context: &RelayRequestContext,
) -> Result<()> {
    let requested_upstream =
        plan.upstream.ok_or_else(|| anyhow!("tee relay expected an upstream segment"))?;

    match stream_tee_upstream(
        writer,
        http_client,
        &spec.upstream.url,
        spec.upstream.content_length,
        prefix_target,
        context,
        requested_upstream,
    ) {
        Ok(_) => {}
        Err(err) => {
            cleanup_incomplete_tee_prefix(prefix_target, context);

            let Some(failure) = relay_stream_failure(&err) else {
                return Err(err);
            };
            if failure.scope != RelayStreamFailureScope::Upstream {
                return Err(err);
            }

            let resume_start = requested_upstream.start + failure.bytes_written;
            if resume_start < requested_upstream.end {
                stream_upstream_with_fresh_url_recovery(
                    writer,
                    http_client,
                    strategy,
                    spec,
                    super::RelayByteRange { start: resume_start, end: requested_upstream.end },
                    url_resolver,
                    context,
                )?;
            }
        }
    }

    Ok(())
}

fn stream_cache_hit_response(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    strategy: &RelayPlayStrategy,
    spec: &mut RelaySessionSpec,
    plan: &super::RelayResponsePlan,
    url_resolver: Option<&UrlResolver>,
    context: &RelayRequestContext,
) -> Result<()> {
    if let Some(staged) = plan.staged {
        stream_staged_segment(writer, &spec.staged.path, staged.start, staged.len())?;
    }
    if let Some(upstream) = plan.upstream {
        stream_upstream_with_fresh_url_recovery(
            writer,
            http_client,
            strategy,
            spec,
            upstream,
            url_resolver,
            context,
        )?;
    }

    Ok(())
}

fn stream_upstream_with_fresh_url_recovery(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    strategy: &RelayPlayStrategy,
    spec: &mut RelaySessionSpec,
    requested_range: super::RelayByteRange,
    url_resolver: Option<&UrlResolver>,
    context: &RelayRequestContext,
) -> Result<()> {
    let mut resume_start = requested_range.start;
    let end = requested_range.end;
    let mut refreshed_once = false;

    while resume_start < end {
        let plans = strategy.continuation_plans(resume_start, end);
        let result = stream_upstream_with_recovery_plans(
            writer,
            http_client,
            &spec.upstream.url,
            plans.as_slice(),
            context,
        );

        match result {
            Ok(_bytes_written) => {
                break;
            }
            Err(err) => {
                let Some(failure) = relay_stream_failure(&err) else {
                    return Err(err);
                };

                if failure.scope != RelayStreamFailureScope::Upstream {
                    return Err(err);
                }

                resume_start += failure.bytes_written;
                if resume_start >= end {
                    break;
                }

                if failure.bytes_written > 0 {
                    log::info!(
                        "[RELAY] upstream progress before retry: session={} track={} resumed_at={} remaining={} host={} url={}",
                        context.session_id,
                        context.track_id,
                        resume_start,
                        end - resume_start,
                        upstream_host(&spec.upstream.url),
                        spec.upstream.url,
                    );
                    continue;
                }

                if refreshed_once {
                    return Err(err);
                }

                let Some(url_resolver) = url_resolver else {
                    return Err(err);
                };

                let old_url = spec.upstream.url.clone();
                let old_host = upstream_host(&old_url);
                let fresh_url = url_resolver
                    .refresh_url_with_policy_forced(&spec.track_id)
                    .map_err(|refresh_err| {
                    relay_failure(
                        RelayStreamFailureKind::Retryable,
                        format!(
                            "force refresh relay URL after fallback exhaustion failed at offset {}: {}",
                            resume_start, refresh_err,
                        ),
                    )
                })?;

                log::info!(
                    "[RELAY] forcing fresh URL after fallback exhaustion: session={} track={} offset={} old_host={} new_host={} old_url={} new_url={}",
                    context.session_id,
                    context.track_id,
                    resume_start,
                    old_host,
                    upstream_host(&fresh_url),
                    old_url,
                    fresh_url,
                );

                spec.upstream.url = fresh_url;
                refreshed_once = true;
            }
        }
    }

    Ok(())
}

fn stream_staged_segment(writer: &mut TcpStream, path: &Path, start: u64, len: u64) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open staged artifact {}", path.display()))?;
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("seek staged artifact {}", path.display()))?;
    copy_exact(&mut file, writer, len)?;
    Ok(())
}

/// Tee mode: opens ONE upstream connection, writes first `tee.size` bytes to
/// `tee.path` on disk (prefix cache), and pipes everything to MPV.
fn stream_tee_upstream(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    content_length: u64,
    tee: &super::RelayTeePrefix,
    context: &RelayRequestContext,
    requested_range: super::RelayByteRange,
) -> Result<u64> {
    let start_byte = requested_range.start;
    let end_byte = requested_range.end;
    let mut remaining = requested_range.len();
    let prefix_already_cached =
        std::fs::metadata(&tee.path).ok().map(|m| m.len().min(tee.size)).unwrap_or(0);
    let should_rewrite_prefix = start_byte == 0 && tee.size > 0;
    let can_append_prefix =
        start_byte > 0 && start_byte == prefix_already_cached && start_byte < tee.size;
    let should_cache_prefix = should_rewrite_prefix || can_append_prefix;

    log::info!(
        "[RELAY] tee mode: session={} track={} host={} peer={:?} content_length={} requested={}..{} prefix_cached={} should_cache_prefix={} url={}",
        context.session_id,
        context.track_id,
        context.upstream_host,
        context.peer_addr,
        content_length,
        start_byte,
        end_byte,
        prefix_already_cached,
        should_cache_prefix,
        context.upstream_url,
    );

    if !should_cache_prefix && start_byte < tee.size {
        log::warn!(
            "[RELAY] tee prefix caching disabled for non-contiguous resume: session={} track={} client_range={:?} start_byte={} prefix_cached={} path={} url={}",
            context.session_id,
            context.track_id,
            context.client_range,
            start_byte,
            prefix_already_cached,
            tee.path.display(),
            context.upstream_url,
        );
    }

    let range_end = end_byte.saturating_sub(1);
    let mut response = http_client
        .get(upstream_url)
        .header(reqwest::header::RANGE, format!("bytes={start_byte}-{range_end}"))
        .send()
        .map_err(|err| {
            relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                0,
                format!("tee upstream request to {upstream_url}: {err}"),
            )
        })?;

    let status = response.status();
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    validate_tee_upstream_response(status, content_range, requested_range, content_length)?;

    if should_cache_prefix {
        if let Some(parent) = tee.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create prefix cache dir {}", parent.display()))?;
        }
    }

    let mut prefix_file =
        if should_cache_prefix {
            if should_rewrite_prefix {
                Some(
                    std::fs::File::create(&tee.path)
                        .with_context(|| format!("create prefix file {}", tee.path.display()))?,
                )
            } else {
                Some(std::fs::OpenOptions::new().append(true).open(&tee.path).with_context(
                    || format!("open prefix file for append {}", tee.path.display()),
                )?)
            }
        } else {
            None
        };

    let mut buf = [0u8; 32768];
    let mut total_written: u64 = 0;
    let mut prefix_written: u64 = if should_rewrite_prefix { 0 } else { prefix_already_cached };
    let tee_size = tee.size;

    while remaining > 0 {
        let chunk_len = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let n = response.read(&mut buf[..chunk_len]).map_err(|err| {
            relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                total_written,
                format!("read from tee upstream: {err}"),
            )
        })?;
        if n == 0 {
            return Err(relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                total_written,
                format!(
                    "unexpected EOF from tee upstream: copied {} of {} bytes for bytes={start_byte}-{range_end}",
                    total_written,
                    requested_range.len(),
                ),
            ));
        }

        // Write to prefix file if we haven't filled it yet
        if let Some(ref mut pfile) = prefix_file {
            if prefix_written < tee_size {
                let to_cache = ((tee_size - prefix_written) as usize).min(n);
                pfile.write_all(&buf[..to_cache]).with_context(|| "write to prefix file")?;
                prefix_written += to_cache as u64;

                if prefix_written >= tee_size {
                    pfile.flush()?;
                    log::info!(
                        "[RELAY] tee: session={} track={} prefix cache complete ({} bytes saved to {})",
                        context.session_id,
                        context.track_id,
                        prefix_written,
                        tee.path.display()
                    );
                }
            }
        }

        let written =
            write_all_to_client(writer, &buf[..n], total_written, "write to MPV downstream")?;
        total_written += written;
        remaining -= written;
    }

    log::info!(
        "[RELAY] tee complete: session={} track={} host={} total_bytes={} prefix_cached={} requested_len={}",
        context.session_id,
        context.track_id,
        context.upstream_host,
        total_written,
        prefix_written,
        requested_range.len(),
    );

    Ok(total_written)
}

fn cleanup_incomplete_tee_prefix(tee: &super::RelayTeePrefix, context: &RelayRequestContext) {
    let prefix_bytes = match std::fs::metadata(&tee.path) {
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            log::warn!(
                "[RELAY] failed to inspect tee prefix after error: session={} track={} path={} err={}",
                context.session_id,
                context.track_id,
                tee.path.display(),
                err,
            );
            return;
        }
    };

    if prefix_bytes >= tee.size {
        return;
    }

    match std::fs::remove_file(&tee.path) {
        Ok(()) => log::info!(
            "[RELAY] dropped incomplete tee prefix after stream failure: session={} track={} bytes={} target={} path={}",
            context.session_id,
            context.track_id,
            prefix_bytes,
            tee.size,
            tee.path.display(),
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log::warn!(
            "[RELAY] failed to drop incomplete tee prefix after stream failure: session={} track={} bytes={} target={} path={} err={}",
            context.session_id,
            context.track_id,
            prefix_bytes,
            tee.size,
            tee.path.display(),
            err,
        ),
    }
}

fn validate_tee_upstream_response(
    status: reqwest::StatusCode,
    content_range: Option<&str>,
    requested_range: super::RelayByteRange,
    content_length: u64,
) -> Result<()> {
    let expected_content_range =
        format!("bytes {}-{}/{}", requested_range.start, requested_range.end - 1, content_length);

    if requested_range.start == 0 && requested_range.end == content_length {
        if status == reqwest::StatusCode::OK || status == reqwest::StatusCode::PARTIAL_CONTENT {
            return Ok(());
        }

        return Err(relay_failure(
            classify_upstream_status(status),
            format!(
                "tee upstream returned {status}, expected 200/206 for full-range request bytes=0-{}",
                content_length.saturating_sub(1)
            ),
        ));
    }

    if status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(relay_failure(
            RelayStreamFailureKind::Terminal,
            format!(
                "tee upstream ignored partial range request bytes={}-{}: returned {status}",
                requested_range.start,
                requested_range.end - 1,
            ),
        ));
    }

    if content_range != Some(expected_content_range.as_str()) {
        return Err(relay_failure(
            RelayStreamFailureKind::Terminal,
            format!(
                "tee upstream returned unexpected Content-Range {:?}, expected {}",
                content_range, expected_content_range,
            ),
        ));
    }

    Ok(())
}

fn stream_upstream_with_recovery_plans(
    writer: &mut TcpStream,
    http_client: &reqwest::blocking::Client,
    upstream_url: &str,
    plans: &[UpstreamReadPlan],
    context: &RelayRequestContext,
) -> Result<u64> {
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
            Ok(bytes_written) => return Ok(bytes_written),
            Err(err) => {
                let (failure_scope, bytes_written) = relay_stream_failure(&err)
                    .map(|failure| (failure.scope, failure.bytes_written))
                    .unwrap_or((RelayStreamFailureScope::Local, 0));
                log::warn!(
                    "[RELAY] upstream retry failed: session={} track={} host={} strategy={} client_range={:?} attempt={}/{} plan={:?} elapsed_ms={} bytes_written={} scope={:?} url={} err={}",
                    context.session_id,
                    context.track_id,
                    upstream_host(upstream_url),
                    context.strategy,
                    context.client_range,
                    attempt_idx + 1,
                    plans.len(),
                    plan,
                    started.elapsed().as_millis(),
                    bytes_written,
                    failure_scope,
                    upstream_url,
                    err,
                );

                if failure_scope == RelayStreamFailureScope::Upstream && bytes_written > 0 {
                    return Err(err);
                }

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
) -> Result<u64> {
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
) -> Result<u64> {
    let query_end = end.saturating_sub(1);
    let ranged_url = format!("{upstream_url}&range={start}-{query_end}");
    let mut response = http_client.get(&ranged_url).send().map_err(|err| {
        relay_failure_with_scope(
            RelayStreamFailureScope::Upstream,
            RelayStreamFailureKind::Retryable,
            0,
            format!("single query-range request to {ranged_url}: {err}"),
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(relay_failure(
            classify_upstream_status(status),
            format!("single query-range request returned {status} for bytes={start}-{query_end}"),
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
) -> Result<u64> {
    let query_end = end.saturating_sub(1);
    let ranged_url = format!("{upstream_url}&range=0-{query_end}");
    let mut response = http_client.get(&ranged_url).send().map_err(|err| {
        relay_failure_with_scope(
            RelayStreamFailureScope::Upstream,
            RelayStreamFailureKind::Retryable,
            0,
            format!("anchored query-range request to {ranged_url}: {err}"),
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(relay_failure(
            classify_upstream_status(status),
            format!("anchored query-range request returned {status} for bytes=0-{query_end}"),
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
) -> Result<u64> {
    let total = end.saturating_sub(start);
    let mut pos = start;
    let mut chunk_num = 0u32;
    let mut total_written = 0u64;

    while pos < end {
        let chunk_end = (pos + RANGE_CHUNK_SIZE).min(end).saturating_sub(1);
        let ranged_url = format!("{upstream_url}&range={pos}-{chunk_end}&rn={chunk_num}");
        log::debug!("[RELAY] chunked &range={}-{} (chunk {})", pos, chunk_end, chunk_num);

        let mut response = http_client.get(&ranged_url).send().map_err(|err| {
            relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                total_written,
                format!("chunked range request chunk {chunk_num}: {err}"),
            )
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(relay_failure(
                classify_upstream_status(status),
                format!("chunked &range= returned {status} at chunk {chunk_num} (pos={pos})"),
            ));
        }

        let chunk_len = chunk_end - pos + 1;
        let mut remaining = chunk_len;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let to_read = (remaining as usize).min(buf.len());
            let n = response.read(&mut buf[..to_read]).map_err(|err| {
                relay_failure_with_scope(
                    RelayStreamFailureScope::Upstream,
                    RelayStreamFailureKind::Retryable,
                    total_written,
                    format!("read from chunked upstream: {err}"),
                )
            })?;
            if n == 0 {
                return Err(relay_failure_with_scope(
                    RelayStreamFailureScope::Upstream,
                    RelayStreamFailureKind::Retryable,
                    total_written,
                    format!(
                        "unexpected EOF from chunked upstream: chunk={} copied={} expected_chunk_bytes={} global_range={}-{}",
                        chunk_num,
                        chunk_len.saturating_sub(remaining),
                        chunk_len,
                        start,
                        end.saturating_sub(1),
                    ),
                ));
            }
            let wrote =
                write_all_to_client(writer, &buf[..n], total_written, "write chunked data to MPV")?;
            total_written += wrote;
            remaining -= wrote;
        }

        pos = chunk_end + 1;
        chunk_num += 1;
    }

    log::info!("[RELAY] chunked &range= complete: {} bytes in {} chunks", total, chunk_num);
    Ok(total_written)
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

fn copy_exact(reader: &mut impl Read, writer: &mut impl Write, len: u64) -> Result<u64> {
    let mut remaining = len;
    let mut copied = 0u64;
    let mut buf = [0_u8; 64 * 1024];

    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = reader.read(&mut buf[..chunk]).map_err(|err| {
            relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                copied,
                format!("read relay segment: {err}"),
            )
        })?;
        if read == 0 {
            return Err(relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                copied,
                format!(
                    "unexpected EOF while streaming relay segment: copied {} of {} bytes (remaining {})",
                    copied, len, remaining,
                ),
            ));
        }

        let written =
            write_all_to_client(writer, &buf[..read], copied, "write relay segment to client")?;
        copied += written;
        remaining -= written;
    }

    Ok(copied)
}

fn discard_exact(reader: &mut impl Read, len: u64) -> Result<()> {
    let mut remaining = len;
    let mut discarded = 0u64;
    let mut buf = [0_u8; 64 * 1024];

    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = reader.read(&mut buf[..chunk]).map_err(|err| {
            relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                0,
                format!("discard relay prefix bytes: {err}"),
            )
        })?;
        if read == 0 {
            return Err(relay_failure_with_scope(
                RelayStreamFailureScope::Upstream,
                RelayStreamFailureKind::Retryable,
                0,
                format!(
                    "unexpected EOF while discarding relay prefix bytes: discarded {} of {} bytes (remaining {})",
                    discarded, len, remaining,
                ),
            ));
        }
        let read = u64::try_from(read).unwrap_or(0);
        discarded += read;
        remaining -= read;
    }

    Ok(())
}

fn write_all_to_client(
    writer: &mut impl Write,
    buf: &[u8],
    bytes_written: u64,
    context: &str,
) -> Result<u64> {
    let mut offset = 0usize;
    while offset < buf.len() {
        match writer.write(&buf[offset..]) {
            Ok(0) => {
                return Err(relay_failure_with_scope(
                    RelayStreamFailureScope::Downstream,
                    RelayStreamFailureKind::Retryable,
                    bytes_written + offset as u64,
                    format!("{context}: wrote 0 bytes"),
                ));
            }
            Ok(written) => {
                offset += written;
            }
            Err(err) => {
                return Err(relay_failure_with_scope(
                    RelayStreamFailureScope::Downstream,
                    RelayStreamFailureKind::Retryable,
                    bytes_written + offset as u64,
                    format!("{context}: {err}"),
                ));
            }
        }
    }

    Ok(offset as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayStreamFailureKind {
    Retryable,
    ExpiredUrl,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayStreamFailureScope {
    Upstream,
    Downstream,
    Local,
}

#[derive(Debug)]
struct RelayStreamFailure {
    kind: RelayStreamFailureKind,
    scope: RelayStreamFailureScope,
    bytes_written: u64,
    message: String,
}

impl std::fmt::Display for RelayStreamFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for RelayStreamFailure {}

fn relay_failure(kind: RelayStreamFailureKind, message: impl Into<String>) -> anyhow::Error {
    relay_failure_with_scope(RelayStreamFailureScope::Upstream, kind, 0, message)
}

fn relay_failure_with_scope(
    scope: RelayStreamFailureScope,
    kind: RelayStreamFailureKind,
    bytes_written: u64,
    message: impl Into<String>,
) -> anyhow::Error {
    anyhow::Error::new(RelayStreamFailure { kind, scope, bytes_written, message: message.into() })
}

fn relay_stream_failure(err: &anyhow::Error) -> Option<RelayStreamFailure> {
    err.chain().find_map(|cause| {
        cause.downcast_ref::<RelayStreamFailure>().map(|failure| RelayStreamFailure {
            kind: failure.kind,
            scope: failure.scope,
            bytes_written: failure.bytes_written,
            message: failure.message.clone(),
        })
    })
}

fn classify_upstream_status(status: reqwest::StatusCode) -> RelayStreamFailureKind {
    if status == reqwest::StatusCode::FORBIDDEN {
        RelayStreamFailureKind::ExpiredUrl
    } else if status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        RelayStreamFailureKind::Retryable
    } else {
        RelayStreamFailureKind::Terminal
    }
}

fn classify_stream_error(err: &anyhow::Error) -> RelayStreamFailureKind {
    for cause in err.chain() {
        if let Some(failure) = cause.downcast_ref::<RelayStreamFailure>() {
            return failure.kind;
        }

        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof => return RelayStreamFailureKind::Retryable,
                _ => {}
            }
        }

        if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>() {
            if let Some(status) = reqwest_err.status() {
                return classify_upstream_status(status);
            }

            if reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_body() {
                return RelayStreamFailureKind::Retryable;
            }
        }
    }

    RelayStreamFailureKind::Terminal
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
        extract_session_id_from_path, promote_tee_prefix_to_cache, stream_tee_response,
        stream_tee_upstream, stream_upstream_segment, stream_upstream_with_fresh_url_recovery,
        stream_upstream_with_recovery_plans,
    };
    use crate::backends::youtube::{
        audio::{CacheConfig, cache::AudioCache},
        config::ExtractorType,
        extractor::Extractor,
        media::{
            RelayByteRange, RelayPlayStrategy, RelayResponsePlan, RelaySessionSpec,
            RelaySessionState, RelayStagedArtifact, RelayTeePrefix, RelayTransportContract,
            RelayUpstreamStream, UpstreamReadPlan,
        },
        url_resolver::UrlResolver,
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
            upstream_url: "https://example.com/videoplayback?id=track-123".to_string(),
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

    #[test]
    fn tee_stream_respects_bounded_range_requests() {
        let observed_range = Arc::new(std::sync::Mutex::new(None::<String>));
        let observed_range_server = Arc::clone(&observed_range);
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = upstream_listener.accept().expect("accept upstream connection");
            let request = read_http_request(&mut stream);
            let range_header = request.lines().find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    (name.eq_ignore_ascii_case("range")).then(|| value.trim().to_string())
                })
            });
            *observed_range_server.lock().expect("lock observed range") = range_header;

            let body: Vec<u8> = (100..150).map(|i| (i % 251) as u8).collect();
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 100-149/1000\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).expect("write response headers");
            stream.write_all(&body).expect("write response body");
        });

        let temp_dir = TempDir::new().expect("create temp dir");
        let tee = RelayTeePrefix { path: temp_dir.path().join("prefix.webm"), size: 200 };
        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");
        let (mut writer, mut reader) = tcp_pair();

        stream_tee_upstream(
            &mut writer,
            &client,
            &upstream_url,
            1_000,
            &tee,
            &test_request_context(),
            RelayByteRange { start: 100, end: 150 },
        )
        .expect("stream bounded tee range");

        writer.shutdown(Shutdown::Write).expect("shutdown writer");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).expect("read streamed bytes");

        assert_eq!(streamed.len(), 50);
        assert_eq!(
            observed_range.lock().expect("lock observed range").as_deref(),
            Some("bytes=100-149")
        );
        assert!(!tee.path.exists(), "non-contiguous resume should not extend tee cache");

        server.join().expect("join upstream server");
    }

    #[test]
    fn tee_stream_does_not_append_non_contiguous_prefix_bytes() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = upstream_listener.accept().expect("accept upstream connection");
            let _request = read_http_request(&mut stream);
            let body: Vec<u8> = (20..40).map(|i| (i % 251) as u8).collect();
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 20-39/1000\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).expect("write response headers");
            stream.write_all(&body).expect("write response body");
        });

        let temp_dir = TempDir::new().expect("create temp dir");
        let tee_path = temp_dir.path().join("prefix.webm");
        std::fs::write(&tee_path, vec![7u8; 10]).expect("seed prefix file");
        let tee = RelayTeePrefix { path: tee_path.clone(), size: 200 };
        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");
        let (mut writer, _reader) = tcp_pair();

        stream_tee_upstream(
            &mut writer,
            &client,
            &upstream_url,
            1_000,
            &tee,
            &test_request_context(),
            RelayByteRange { start: 20, end: 40 },
        )
        .expect("stream non-contiguous tee range");

        let prefix = std::fs::read(&tee_path).expect("read prefix file");
        assert_eq!(prefix, vec![7u8; 10]);

        server.join().expect("join upstream server");
    }

    #[test]
    fn tee_failure_drops_incomplete_prefix_and_continues_uncached() {
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
                handled += 1;

                let request = read_http_request(&mut stream);
                let request_line = request.lines().next().unwrap_or_default().to_string();

                if handled == 1 {
                    let body: Vec<u8> = (0..20).map(|i| (i % 251) as u8).collect();
                    let headers = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-49/1000\r\nConnection: close\r\n\r\n",
                        50
                    );
                    stream.write_all(headers.as_bytes()).expect("write first response headers");
                    stream.write_all(&body).expect("write partial tee body");
                    stream.shutdown(Shutdown::Both).ok();
                    continue;
                }

                let (start, end) =
                    parse_query_range(&request_line).expect("continuation query range");
                assert_eq!((start, end), (20, 49));
                let body: Vec<u8> = (start..=end).map(|i| (i % 251) as u8).collect();
                write_http_response(&mut stream, "200 OK", &body);
                break;
            }
        });

        let temp_dir = TempDir::new().expect("create temp dir");
        let prefix_path = temp_dir.path().join("prefix.webm");
        let prefix = RelayTeePrefix { path: prefix_path.clone(), size: 40 };
        let upstream_url = format!("http://{upstream_addr}/videoplayback?foo=bar");
        let client = reqwest::blocking::Client::builder().build().expect("build client");
        let (mut writer, mut reader) = tcp_pair();
        let strategy = RelayPlayStrategy::TeeMissRelay {
            track_id: "track-123".to_string(),
            stream_url: upstream_url.clone(),
            prefix_target: prefix.clone(),
        };
        let mut spec = RelaySessionSpec {
            track_id: "track-123".to_string(),
            staged: RelayStagedArtifact {
                path: temp_dir.path().join("staged.webm"),
                available: RelayByteRange { start: 0, end: 0 },
            },
            upstream: RelayUpstreamStream { url: upstream_url, content_length: 1_000 },
            contract: RelayTransportContract::default(),
            state: RelaySessionState::AwaitingRequest,
            tee_prefix: Some(prefix.clone()),
        };
        let plan = RelayResponsePlan {
            response: RelayByteRange { start: 0, end: 50 },
            staged: None,
            upstream: Some(RelayByteRange { start: 0, end: 50 }),
            content_length: 1_000,
        };

        stream_tee_response(
            &mut writer,
            &client,
            &strategy,
            &mut spec,
            &plan,
            &prefix,
            None,
            &test_request_context(),
        )
        .expect("tee response should continue uncached after partial failure");

        writer.shutdown(Shutdown::Write).expect("shutdown writer");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).expect("read streamed bytes");

        let expected: Vec<u8> = (0..50).map(|i| (i % 251) as u8).collect();
        assert_eq!(streamed, expected);
        assert!(!prefix_path.exists(), "incomplete tee prefix should be deleted");

        server.join().expect("join upstream server");
    }

    #[test]
    fn promote_tee_prefix_skips_missing_or_incomplete_prefix() {
        let temp_dir = TempDir::new().expect("tempdir");
        let cache = AudioCache::new(CacheConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            prefix_size: 204_800,
            max_cache_size: 209_715_200,
        })
        .expect("cache");
        let prefix_path = temp_dir.path().join("track-skip.webm");

        let strategy = RelayPlayStrategy::TeeMissRelay {
            track_id: "track-skip".to_string(),
            stream_url: "https://example.com/stream".to_string(),
            prefix_target: RelayTeePrefix { path: prefix_path.clone(), size: 1024 },
        };
        let spec = RelaySessionSpec {
            track_id: "track-skip".to_string(),
            staged: RelayStagedArtifact {
                path: temp_dir.path().join("staged.webm"),
                available: RelayByteRange { start: 0, end: 0 },
            },
            upstream: RelayUpstreamStream {
                url: "https://example.com/stream".to_string(),
                content_length: 10_000,
            },
            contract: RelayTransportContract::default(),
            state: RelaySessionState::AwaitingRequest,
            tee_prefix: Some(RelayTeePrefix { path: prefix_path.clone(), size: 1024 }),
        };

        promote_tee_prefix_to_cache(&cache, &strategy, &spec)
            .expect("missing prefix should be ignored");

        std::fs::write(&prefix_path, vec![1u8; 512]).expect("write partial prefix");
        promote_tee_prefix_to_cache(&cache, &strategy, &spec)
            .expect("incomplete prefix should be ignored");
        assert!(cache.get_prefix_metadata("track-skip").is_none());
    }

    #[derive(Clone)]
    struct FreshUrlExtractor {
        stale_url: String,
        fresh_url: String,
        fresh_calls: Arc<AtomicUsize>,
    }

    impl Extractor for FreshUrlExtractor {
        fn extract_batch(
            &self,
            video_ids: &[String],
        ) -> std::collections::HashMap<String, anyhow::Result<String>> {
            video_ids
                .iter()
                .map(|video_id| (video_id.clone(), self.extract_one(video_id)))
                .collect()
        }

        fn name(&self) -> &'static str {
            "fresh-url-extractor"
        }

        fn extract_one(&self, _video_id: &str) -> anyhow::Result<String> {
            Ok(self.stale_url.clone())
        }

        fn extract_one_fresh(&self, _video_id: &str) -> anyhow::Result<String> {
            self.fresh_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.fresh_url.clone())
        }

        fn clear_cache(&self) {}

        fn is_cached(&self, _video_id: &str) -> bool {
            false
        }

        fn invalidate(&self, _video_id: &str) {}
    }

    #[test]
    fn fresh_url_retry_resumes_from_last_streamed_byte_after_plan_exhaustion() {
        let stale_requests = Arc::new(AtomicUsize::new(0));
        let stale_requests_server = Arc::clone(&stale_requests);
        let fresh_requests = Arc::new(AtomicUsize::new(0));
        let fresh_requests_server = Arc::clone(&fresh_requests);
        let fresh_calls = Arc::new(AtomicUsize::new(0));
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream listener");
        let upstream_addr = upstream_listener.local_addr().expect("upstream local addr");
        upstream_listener.set_nonblocking(true).expect("set upstream listener nonblocking");

        let server = thread::spawn(move || {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(3) {
                let (mut stream, _) = match upstream_listener.accept() {
                    Ok(conn) => conn,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };

                let request = read_http_request(&mut stream);
                let request_line = request.lines().next().unwrap_or_default().to_string();
                let target = request_line.split_whitespace().nth(1).unwrap_or_default().to_string();
                let range = parse_query_range(&request_line).expect("query range");

                if target.contains("/stale") {
                    let stale_idx = stale_requests_server.fetch_add(1, Ordering::SeqCst);
                    match stale_idx {
                        0 => {
                            let body: Vec<u8> =
                                (range.0..range.0 + 20).map(|i| (i % 251) as u8).collect();
                            write_http_response(&mut stream, "200 OK", &body);
                        }
                        1 | 2 | 3 => {
                            write_http_response(&mut stream, "403 Forbidden", b"");
                        }
                        _ => break,
                    }
                    continue;
                }

                if target.contains("/fresh") {
                    fresh_requests_server.fetch_add(1, Ordering::SeqCst);
                    let body: Vec<u8> = (range.0..=range.1).map(|i| (i % 251) as u8).collect();
                    write_http_response(&mut stream, "200 OK", &body);
                    break;
                }
            }
        });

        let stale_url = format!("http://{upstream_addr}/stale?videoplayback=1");
        let fresh_url = format!("http://{upstream_addr}/fresh?videoplayback=1");
        let resolver = UrlResolver::from_extractor(
            Arc::new(FreshUrlExtractor {
                stale_url: stale_url.clone(),
                fresh_url: fresh_url.clone(),
                fresh_calls: Arc::clone(&fresh_calls),
            }),
            ExtractorType::Ytx,
        );

        let client = reqwest::blocking::Client::builder().build().expect("build client");
        let (mut writer, mut reader) = tcp_pair();
        let requested_range = RelayByteRange { start: 100, end: 150 };
        let strategy = RelayPlayStrategy::CacheHitRelay {
            track_id: "track-123".to_string(),
            stream_url: stale_url.clone(),
            prefix: RelayStagedArtifact {
                path: PathBuf::from("/tmp/prefix.webm"),
                available: RelayByteRange { start: 0, end: 100 },
            },
        };
        let mut spec = RelaySessionSpec {
            track_id: "track-123".to_string(),
            staged: RelayStagedArtifact {
                path: PathBuf::from("/tmp/prefix.webm"),
                available: RelayByteRange { start: 0, end: 100 },
            },
            upstream: RelayUpstreamStream { url: stale_url, content_length: 1_000 },
            contract: RelayTransportContract::default(),
            state: RelaySessionState::AwaitingRequest,
            tee_prefix: None,
        };

        stream_upstream_with_fresh_url_recovery(
            &mut writer,
            &client,
            &strategy,
            &mut spec,
            requested_range,
            Some(&resolver),
            &test_request_context(),
        )
        .expect("fresh url recovery should succeed");

        writer.shutdown(Shutdown::Write).expect("shutdown writer");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).expect("read streamed bytes");

        let expected: Vec<u8> = (100..150).map(|i| (i % 251) as u8).collect();
        assert_eq!(streamed, expected);
        assert_eq!(stale_requests.load(Ordering::SeqCst), 4);
        assert_eq!(fresh_requests.load(Ordering::SeqCst), 1);
        assert_eq!(fresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(spec.upstream.url, fresh_url);

        server.join().expect("join upstream server");
    }
}
