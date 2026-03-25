//! yt-dlp extractor - reliable fallback using yt-dlp CLI.
use std::{
    collections::HashMap,
    fs::OpenOptions,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;

use super::Extractor;
use crate::backends::youtube::config::YtDlpExtractorConfig;

const BGUTIL_HOST: &str = "127.0.0.1";
const BGUTIL_PORT: u16 = 4417;
const BGUTIL_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const BGUTIL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const BGUTIL_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const BGUTIL_LOG_PATH: &str = "/tmp/yrmpc-bgutil-pot-provider.log";
const BGUTIL_SCRIPT_ENV_VAR: &str = "YRMPC_BGUTIL_SERVER_PATH";
const JS_RUNTIME_ENV_VAR: &str = "YRMPC_YTDLP_JS_RUNTIME";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderEndpoint {
    host: String,
    port: u16,
}

impl Default for ProviderEndpoint {
    fn default() -> Self {
        Self { host: BGUTIL_HOST.to_string(), port: BGUTIL_PORT }
    }
}

impl ProviderEndpoint {
    fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    fn is_reachable(&self) -> bool {
        let Ok(addrs) = (self.host.as_str(), self.port).to_socket_addrs() else {
            return false;
        };

        addrs
            .into_iter()
            .any(|addr| TcpStream::connect_timeout(&addr, BGUTIL_CONNECT_TIMEOUT).is_ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsRuntimeKind {
    Bun,
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsRuntime {
    kind: JsRuntimeKind,
    executable: PathBuf,
}

impl JsRuntime {
    fn bun(path: impl Into<PathBuf>) -> Self {
        Self { kind: JsRuntimeKind::Bun, executable: path.into() }
    }

    fn node(path: impl Into<PathBuf>) -> Self {
        Self { kind: JsRuntimeKind::Node, executable: path.into() }
    }

    fn from_path(path: PathBuf) -> Self {
        let kind = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| {
                if name.contains("bun") {
                    JsRuntimeKind::Bun
                } else {
                    JsRuntimeKind::Node
                }
            })
            .unwrap_or(JsRuntimeKind::Node);

        Self { kind, executable: path }
    }

    fn executable(&self) -> &Path {
        &self.executable
    }

    fn yt_dlp_arg(&self) -> String {
        match self.kind {
            JsRuntimeKind::Bun => format!("bun:{}", self.executable.display()),
            JsRuntimeKind::Node => format!("node:{}", self.executable.display()),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct JsRuntimeResolver;

impl JsRuntimeResolver {
    fn resolve(&self) -> Result<JsRuntime> {
        let explicit = std::env::var_os(JS_RUNTIME_ENV_VAR).map(PathBuf::from);
        let bun = which::which("bun").ok();
        let node = which::which("node").ok();

        Self::select(explicit, bun, node).ok_or_else(|| {
            anyhow!(
                "No JavaScript runtime found for yt-dlp PO token support; set {JS_RUNTIME_ENV_VAR} or install bun/node"
            )
        })
    }

    fn select(
        explicit: Option<PathBuf>,
        bun: Option<PathBuf>,
        node: Option<PathBuf>,
    ) -> Option<JsRuntime> {
        explicit
            .map(JsRuntime::from_path)
            .or_else(|| bun.map(JsRuntime::bun))
            .or_else(|| node.map(JsRuntime::node))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PoTokenContext {
    endpoint: ProviderEndpoint,
    runtime: JsRuntime,
}

#[derive(Debug)]
struct PoTokenProviderManager {
    child: Mutex<Option<Child>>,
    endpoint: ProviderEndpoint,
    js_runtime_resolver: JsRuntimeResolver,
}

#[cfg(test)]
pub(crate) static EAGER_BOOTSTRAP_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Default for PoTokenProviderManager {
    fn default() -> Self {
        Self {
            child: Mutex::default(),
            endpoint: ProviderEndpoint::default(),
            js_runtime_resolver: JsRuntimeResolver,
        }
    }
}

impl PoTokenProviderManager {
    fn resolve_js_runtime(&self) -> Result<JsRuntime> {
        self.js_runtime_resolver.resolve()
    }

    fn select_provider_launcher(node: Option<PathBuf>) -> Option<PathBuf> {
        node
    }

    fn resolve_provider_launcher(&self) -> Result<PathBuf> {
        Self::select_provider_launcher(which::which("node").ok()).ok_or_else(|| {
            anyhow!(
                "Node.js is required to start the bgutil provider server; install node or start the provider manually"
            )
        })
    }

    fn ensure_ready(&self) -> Result<PoTokenContext> {
        let runtime = self.resolve_js_runtime()?;

        if self.endpoint.is_reachable() {
            return Ok(PoTokenContext {
                endpoint: self.endpoint.clone(),
                runtime,
            });
        }

        let mut child_guard = self.child.lock();
        let stale_child = if let Some(child) = child_guard.as_mut() {
            match child.try_wait().context("Failed checking bgutil provider status")? {
                None if self.endpoint.is_reachable() => {
                    return Ok(PoTokenContext {
                        endpoint: self.endpoint.clone(),
                        runtime,
                    });
                }
                None => {
                    log::warn!(
                        "bgutil provider process is running but unreachable at {}; restarting",
                        self.endpoint.base_url()
                    );
                    child_guard.take()
                }
                Some(status) => {
                    log::warn!("bgutil provider exited with status {status}; restarting");
                    child_guard.take()
                }
            }
        } else {
            None
        };

        if let Some(child) = stale_child {
            stop_child(child);
        }

        if self.endpoint.is_reachable() {
            return Ok(PoTokenContext {
                endpoint: self.endpoint.clone(),
                runtime,
            });
        }

        let script_path = Self::resolve_script_path()?;
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(BGUTIL_LOG_PATH)
            .with_context(|| format!("Failed to open bgutil log at {BGUTIL_LOG_PATH}"))?;
        let stderr = log_file
            .try_clone()
            .with_context(|| format!("Failed to clone bgutil log handle at {BGUTIL_LOG_PATH}"))?;
        let provider_launcher = self.resolve_provider_launcher()?;

        log::info!(
            "Initializing bgutil provider server with {} {} --port {}",
            provider_launcher.display(),
            script_path.display(),
            self.endpoint.port
        );

        let child = Command::new(&provider_launcher)
            .arg(&script_path)
            .arg("--port")
            .arg(self.endpoint.port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to start bgutil provider with {} {}",
                    provider_launcher.display(),
                    script_path.display()
                )
            })?;

        log::info!(
            "Started bgutil provider on {} via {}",
            self.endpoint.base_url(),
            script_path.display()
        );

        *child_guard = Some(child);
        drop(child_guard);

        self.wait_until_reachable()?;

        Ok(PoTokenContext {
            endpoint: self.endpoint.clone(),
            runtime,
        })
    }

    fn wait_until_reachable(&self) -> Result<()> {
        let deadline = std::time::Instant::now() + BGUTIL_STARTUP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if self.endpoint.is_reachable() {
                return Ok(());
            }
            thread::sleep(BGUTIL_POLL_INTERVAL);
        }

        Err(anyhow!(
            "bgutil provider did not become reachable at {}; see {}",
            self.endpoint.base_url(),
            BGUTIL_LOG_PATH
        ))
    }

    fn resolve_script_path() -> Result<PathBuf> {
        let mut candidates = Vec::new();

        if let Ok(path) = std::env::var(BGUTIL_SCRIPT_ENV_VAR) {
            candidates.push(PathBuf::from(path));
        }

        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(
                "workspace/apps/po-token-extract/vendor/bgutil-ytdlp-pot-provider/server/build/main.js",
            ));
        }

        candidates.into_iter().find(|path| path.exists()).ok_or_else(|| {
            anyhow!(
                "bgutil provider script not found; set {BGUTIL_SCRIPT_ENV_VAR} or install it under ~/workspace/apps/po-token-extract/vendor/bgutil-ytdlp-pot-provider/server/build/main.js"
            )
        })
    }
}

impl Drop for PoTokenProviderManager {
    fn drop(&mut self) {
        if let Some(child) = self.child.get_mut().take() {
            stop_child(child);
        }
    }
}

fn stop_child(mut child: Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(err) => {
            log::debug!("Failed checking bgutil child before shutdown: {err}");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum YtDlpAttempt {
    HighQualityWithPoToken,
    Degraded251,
}

impl YtDlpAttempt {
    fn format_selector(self) -> &'static str {
        match self {
            Self::HighQualityWithPoToken => "774/141/251",
            Self::Degraded251 => "251",
        }
    }

    fn requires_provider(self) -> bool {
        matches!(self, Self::HighQualityWithPoToken)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct YtDlpPolicy;

impl YtDlpPolicy {
    fn initial_attempt(&self) -> YtDlpAttempt {
        YtDlpAttempt::HighQualityWithPoToken
    }

    fn degraded_attempt(&self) -> YtDlpAttempt {
        YtDlpAttempt::Degraded251
    }
}

#[derive(Debug, Clone)]
struct YtDlpCommandBuilder {
    cookies_path: Option<String>,
    js_runtime: Option<JsRuntime>,
}

impl YtDlpCommandBuilder {
    fn new(cookies_path: Option<String>, js_runtime: Option<JsRuntime>) -> Self {
        Self { cookies_path, js_runtime }
    }

    fn build(
        &self,
        video_ids: &[String],
        attempt: YtDlpAttempt,
        po_token: Option<&PoTokenContext>,
    ) -> Vec<String> {
        let mut args = vec![
            "-f".to_string(),
            attempt.format_selector().to_string(),
            "--ignore-config".to_string(),
            "--ignore-errors".to_string(),
        ];

        if let Some(runtime) = po_token.map(|po_token| &po_token.runtime).or(self.js_runtime.as_ref()) {
            args.push("--js-runtimes".to_string());
            args.push(runtime.yt_dlp_arg());
        }

        if let Some(po_token) = po_token {
            for extractor_arg in Self::po_token_extractor_args(&po_token.endpoint) {
                args.push("--extractor-args".to_string());
                args.push(extractor_arg);
            }
        }

        if let Some(cookies_path) = &self.cookies_path {
            args.push("--cookies".to_string());
            args.push(cookies_path.clone());
        }

        args.push("--print".to_string());
        args.push("%(id)s\t%(url)s".to_string());
        args.extend(
            video_ids
                .iter()
                .map(|id| format!("https://music.youtube.com/watch?v={id}")),
        );
        args
    }

    fn po_token_extractor_args(endpoint: &ProviderEndpoint) -> Vec<String> {
        vec![
            "youtube:player-client=mweb".to_string(),
            format!("youtubepot-bgutilhttp:base_url={}", endpoint.base_url()),
        ]
    }
}

#[derive(Debug, Default, Clone)]
struct ExtractorDiagnosticsSink;

impl ExtractorDiagnosticsSink {
    fn warn_provider_bootstrap_failure(&self, err: &anyhow::Error) {
        log::warn!(
            "PO-token provider bootstrap failed; retrying yt-dlp once with format 251 and existing cookies: {err}"
        );
    }

    fn debug_stderr(&self, stderr: &str) {
        if !stderr.trim().is_empty() {
            log::debug!("yt-dlp stderr: {}", stderr.trim());
        }
    }
}

#[derive(Debug, Clone)]
pub struct YtDlpExtractor {
    options: YtDlpExtractorConfig,
    policy: YtDlpPolicy,
    provider_manager: Arc<PoTokenProviderManager>,
    diagnostics: ExtractorDiagnosticsSink,
}

impl Default for YtDlpExtractor {
    fn default() -> Self {
        Self {
            options: YtDlpExtractorConfig::default(),
            policy: YtDlpPolicy,
            provider_manager: Arc::new(PoTokenProviderManager::default()),
            diagnostics: ExtractorDiagnosticsSink,
        }
    }
}

impl YtDlpExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(cookies_path: Option<String>) -> Self {
        Self { options: YtDlpExtractorConfig { cookies_path }, ..Self::default() }
    }

    fn command_builder(&self, js_runtime: Option<JsRuntime>) -> YtDlpCommandBuilder {
        YtDlpCommandBuilder::new(self.options.cookies_path.clone(), js_runtime)
    }

    pub(crate) fn bootstrap_po_token_provider_best_effort<F>(&self, ensure_provider: F)
    where
        F: FnOnce() -> Result<()>,
    {
        #[cfg(test)]
        EAGER_BOOTSTRAP_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if let Err(err) = ensure_provider() {
            self.diagnostics.warn_provider_bootstrap_failure(&err);
        }
    }

    pub(crate) fn eager_bootstrap_po_token_provider(&self) {
        self.bootstrap_po_token_provider_best_effort(|| self.provider_manager.ensure_ready().map(|_| ())); 
    }

    #[cfg(test)]
    pub(crate) fn eager_bootstrap_attempts_for_tests() -> usize {
        EAGER_BOOTSTRAP_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn reset_eager_bootstrap_attempts_for_tests() {
        EAGER_BOOTSTRAP_ATTEMPTS.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    fn parse_id_url_lines(stdout: &str) -> HashMap<String, String> {
        let mut by_id = HashMap::new();

        for line in stdout.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if let Some((id, url)) = line.split_once('\t') {
                let id = id.trim();
                let url = url.trim();

                if !id.is_empty() && !url.is_empty() {
                    by_id.insert(id.to_string(), url.to_string());
                }
            }
        }

        by_id
    }

    fn run_yt_dlp(&self, video_ids: &[String], args: Vec<String>) -> HashMap<String, Result<String>> {
        let mut cmd = Command::new("yt-dlp");
        cmd.args(args);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(err) => {
                let msg = format!("Failed to run yt-dlp. Is it installed? {err}");
                return video_ids
                    .iter()
                    .cloned()
                    .map(|id| (id, Err(anyhow!(msg.clone()))))
                    .collect();
            }
        };

        self.map_output(video_ids, output.stdout, output.stderr)
    }

    fn map_output(
        &self,
        video_ids: &[String],
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    ) -> HashMap<String, Result<String>> {
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        let by_id = Self::parse_id_url_lines(&stdout);

        self.diagnostics.debug_stderr(&stderr);

        video_ids
            .iter()
            .cloned()
            .map(|id| {
                let result = if let Some(url) = by_id.get(&id) {
                    log::debug!("yt-dlp extracted URL for {} (len={})", id, url.len());
                    Ok(url.clone())
                } else {
                    let base = format!("No stream URL returned by yt-dlp for {id}");
                    if stderr.trim().is_empty() {
                        Err(anyhow!(base))
                    } else {
                        Err(anyhow!("{base}. stderr: {}", stderr.trim()))
                    }
                };

                (id, result)
            })
            .collect()
    }

    fn extract_bulk_with<F, R>(
        &self,
        video_ids: &[String],
        js_runtime: Option<JsRuntime>,
        ensure_provider: F,
        run_command: R,
    ) -> HashMap<String, Result<String>>
    where
        F: Fn() -> Result<PoTokenContext>,
        R: Fn(Vec<String>) -> HashMap<String, Result<String>>,
    {
        if video_ids.is_empty() {
            return HashMap::new();
        }

        let builder = self.command_builder(js_runtime);
        let initial_attempt = self.policy.initial_attempt();

        if initial_attempt.requires_provider() {
            match ensure_provider() {
                Ok(po_token) => {
                    return run_command(builder.build(video_ids, initial_attempt, Some(&po_token)));
                }
                Err(err) => {
                    self.diagnostics.warn_provider_bootstrap_failure(&err);
                }
            }
        }

        run_command(builder.build(video_ids, self.policy.degraded_attempt(), None))
    }

    fn extract_bulk(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        let js_runtime = self.provider_manager.resolve_js_runtime().ok();
        self.extract_bulk_with(
            video_ids,
            js_runtime,
            || self.provider_manager.ensure_ready(),
            |args| self.run_yt_dlp(video_ids, args),
        )
    }
}

impl Extractor for YtDlpExtractor {
    fn extract_batch(&self, video_ids: &[String]) -> HashMap<String, Result<String>> {
        self.extract_bulk(video_ids)
    }

    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn extract_one(&self, video_id: &str) -> Result<String> {
        let id = video_id.to_string();
        let mut results = self.extract_bulk(std::slice::from_ref(&id));
        results
            .remove(video_id)
            .unwrap_or_else(|| Err(anyhow!("No result returned for {video_id}")))
            .context("yt-dlp extraction failed")
    }

    fn clear_cache(&self) {}

    fn is_cached(&self, _video_id: &str) -> bool {
        false
    }

    fn invalidate(&self, _video_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::cell::RefCell;

    #[test]
    fn test_empty_batch() {
        let extractor = YtDlpExtractor::new();
        let results = extractor.extract_batch(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_id_url_lines() {
        let parsed =
            YtDlpExtractor::parse_id_url_lines("abc\thttps://u1\nignored\nxyz\thttps://u2\n");

        assert_eq!(parsed.get("abc").map(String::as_str), Some("https://u1"));
        assert_eq!(parsed.get("xyz").map(String::as_str), Some("https://u2"));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn provider_endpoint_base_url_is_composed() {
        let endpoint = ProviderEndpoint::default();
        assert_eq!(endpoint.base_url(), "http://127.0.0.1:4417");
    }

    #[test]
    fn js_runtime_resolver_prefers_explicit_then_bun_then_node() {
        let explicit = JsRuntimeResolver::select(
            Some(PathBuf::from("/tmp/custom-node")),
            Some(PathBuf::from("/tmp/bun")),
            Some(PathBuf::from("/tmp/node")),
        )
        .expect("explicit runtime should win");
        assert_eq!(explicit, JsRuntime::node("/tmp/custom-node"));

        let bun = JsRuntimeResolver::select(
            None,
            Some(PathBuf::from("/tmp/bun")),
            Some(PathBuf::from("/tmp/node")),
        )
        .expect("bun should win when explicit runtime missing");
        assert_eq!(bun, JsRuntime::bun("/tmp/bun"));

        let node = JsRuntimeResolver::select(None, None, Some(PathBuf::from("/tmp/node")))
            .expect("node should be used as final fallback");
        assert_eq!(node, JsRuntime::node("/tmp/node"));
    }

    #[test]
    fn provider_launcher_requires_node() {
        let node = PoTokenProviderManager::select_provider_launcher(Some(PathBuf::from("/tmp/node")))
            .expect("node launcher should be selected");
        assert_eq!(node, PathBuf::from("/tmp/node"));
        assert!(PoTokenProviderManager::select_provider_launcher(None).is_none());
    }

    #[test]
    fn initial_policy_attempt_prefers_high_quality_with_po_token() {
        let policy = YtDlpPolicy;
        assert_eq!(policy.initial_attempt(), YtDlpAttempt::HighQualityWithPoToken);
        assert!(policy.initial_attempt().requires_provider());
        assert_eq!(policy.initial_attempt().format_selector(), "774/141/251");
    }

    #[test]
    fn degraded_policy_attempt_uses_251_without_provider() {
        let policy = YtDlpPolicy;
        assert_eq!(policy.degraded_attempt(), YtDlpAttempt::Degraded251);
        assert!(!policy.degraded_attempt().requires_provider());
        assert_eq!(policy.degraded_attempt().format_selector(), "251");
    }

    #[test]
    fn test_build_args_includes_cookies_and_repeated_extractor_args() {
        let builder = YtDlpCommandBuilder::new(Some("/tmp/cookies.txt".to_string()), None);
        let endpoint = ProviderEndpoint::default();
        let context = PoTokenContext {
            endpoint,
            runtime: JsRuntime::bun("/tmp/bun"),
        };

        let args = builder.build(
            &["abc123".to_string()],
            YtDlpAttempt::HighQualityWithPoToken,
            Some(&context),
        );

        assert_eq!(
            args,
            vec![
                "-f",
                "774/141/251",
                "--ignore-config",
                "--ignore-errors",
                "--js-runtimes",
                "bun:/tmp/bun",
                "--extractor-args",
                "youtube:player-client=mweb",
                "--extractor-args",
                "youtubepot-bgutilhttp:base_url=http://127.0.0.1:4417",
                "--cookies",
                "/tmp/cookies.txt",
                "--print",
                "%(id)s\t%(url)s",
                "https://music.youtube.com/watch?v=abc123",
            ]
        );
    }

    #[test]
    fn degraded_args_keep_cookies_and_js_runtime_but_drop_po_token_flags() {
        let builder = YtDlpCommandBuilder::new(
            Some("/tmp/cookies.txt".to_string()),
            Some(JsRuntime::bun("/tmp/bun")),
        );

        let args = builder.build(&["abc123".to_string()], YtDlpAttempt::Degraded251, None);

        assert_eq!(
            args,
            vec![
                "-f",
                "251",
                "--ignore-config",
                "--ignore-errors",
                "--js-runtimes",
                "bun:/tmp/bun",
                "--cookies",
                "/tmp/cookies.txt",
                "--print",
                "%(id)s\t%(url)s",
                "https://music.youtube.com/watch?v=abc123",
            ]
        );
    }

    #[test]
    fn extract_bulk_uses_high_quality_attempt_when_provider_is_ready() {
        let extractor = YtDlpExtractor::with_options(Some("/tmp/cookies.txt".to_string()));
        let calls = RefCell::new(Vec::<Vec<String>>::new());

        let results = extractor.extract_bulk_with(
            &["abc123".to_string()],
            Some(JsRuntime::bun("/tmp/bun")),
            || {
                Ok(PoTokenContext {
                    endpoint: ProviderEndpoint::default(),
                    runtime: JsRuntime::bun("/tmp/bun"),
                })
            },
            |args| {
                calls.borrow_mut().push(args);
                HashMap::from([("abc123".to_string(), Ok("https://stream".to_string()))])
            },
        );

        assert_eq!(results.get("abc123").and_then(|r| r.as_ref().ok()), Some(&"https://stream".to_string()));
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"774/141/251".to_string()));
        assert!(calls[0].contains(&"bun:/tmp/bun".to_string()));
    }

    #[test]
    fn provider_bootstrap_failure_retries_once_with_251() {
        let extractor = YtDlpExtractor::with_options(Some("/tmp/cookies.txt".to_string()));
        let calls = RefCell::new(Vec::<Vec<String>>::new());

        let results = extractor.extract_bulk_with(
            &["abc123".to_string()],
            Some(JsRuntime::bun("/tmp/bun")),
            || Err(anyhow!("bgutil unavailable")),
            |args| {
                calls.borrow_mut().push(args);
                HashMap::from([("abc123".to_string(), Ok("https://fallback".to_string()))])
            },
        );

        assert_eq!(results.get("abc123").and_then(|r| r.as_ref().ok()), Some(&"https://fallback".to_string()));
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"251".to_string()));
        assert!(calls[0].contains(&"--js-runtimes".to_string()));
        assert!(calls[0].contains(&"bun:/tmp/bun".to_string()));
        assert!(!calls[0].iter().any(|arg| arg.starts_with("youtubepot-bgutilhttp:base_url=")));
        assert!(calls[0].contains(&"/tmp/cookies.txt".to_string()));
    }

    #[test]
    fn eager_bootstrap_best_effort_attempts_and_swallows_failure() {
        EAGER_BOOTSTRAP_ATTEMPTS.store(0, Ordering::SeqCst);
        let extractor = YtDlpExtractor::new();
        let called = RefCell::new(false);

        extractor.bootstrap_po_token_provider_best_effort(|| {
            *called.borrow_mut() = true;
            Err(anyhow!("boom"))
        });

        assert!(*called.borrow());
        assert_eq!(EAGER_BOOTSTRAP_ATTEMPTS.load(Ordering::SeqCst), 1);
    }
}
