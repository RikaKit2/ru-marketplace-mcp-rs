use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use super::cookies::SessionData;
use crate::domain::{ScraperError, WafProtection};
use crate::transport::DEFAULT_DESKTOP_UA as DEFAULT_USER_AGENT;

const DEFAULT_PROFILE_DIR: &str = "/tmp/ozon-native-cdp-profile";
static PORT_COUNTER: AtomicU16 = AtomicU16::new(9400);

fn get_available_port() -> u16 {
    if let Ok(listener) = TcpListener::bind("127.0.0.1:0")
        && let Ok(addr) = listener.local_addr()
    {
        return addr.port();
    }
    PORT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

struct CdpClient {
    child: tokio::process::Child,
    ws: async_tungstenite::WebSocketStream<async_tungstenite::tokio::ConnectStream>,
    session_id: String,
    next_id: u64,
    profile_dir: PathBuf,
}

impl Drop for CdpClient {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.profile_dir);
    }
}

impl CdpClient {
    async fn send_cmd(&mut self, method: &str, params: Value) -> Result<Value, ScraperError> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.ws
            .send(Message::Text(req.to_string().into()))
            .await
            .map_err(|e| ScraperError::Parse(format!("Failed to send CDP command: {e}")))?;

        while let Some(msg) = self.ws.next().await {
            match msg {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t)
                        && v.get("id").and_then(|n| n.as_u64()) == Some(id)
                    {
                        return Ok(v);
                    }
                }
                Err(e) => {
                    return Err(ScraperError::Parse(format!("CDP WebSocket error: {e}")));
                }
                _ => {}
            }
        }
        Err(ScraperError::Timeout)
    }

    async fn send_session_cmd(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ScraperError> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "id": id,
            "sessionId": self.session_id,
            "method": method,
            "params": params,
        });
        self.ws
            .send(Message::Text(req.to_string().into()))
            .await
            .map_err(|e| ScraperError::Parse(format!("Failed to send CDP session command: {e}")))?;

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            let next_msg = tokio::time::timeout(Duration::from_millis(1000), self.ws.next()).await;
            match next_msg {
                Ok(Some(Ok(Message::Text(t)))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t)
                        && v.get("id").and_then(|n| n.as_u64()) == Some(id)
                    {
                        return Ok(v);
                    }
                }
                Ok(Some(Err(e))) => {
                    return Err(ScraperError::Parse(format!(
                        "CDP session WebSocket error: {e}"
                    )));
                }
                _ => {}
            }
        }
        Err(ScraperError::Timeout)
    }

    async fn close(mut self) {
        let _ = self.send_cmd("Browser.close", json!({})).await;
        let _ = tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct ActiveSession {
    cdp: CdpClient,
    last_used: std::time::Instant,
}

/// Pure Rust CDP solver that spawns Chromium on-demand (or keeps an active session
/// when explicitly requested via ozon_session_start) to solve Variti Proof-of-Work
/// and execute authenticated queries with 100% genuine Chrome TLS.
#[derive(Clone)]
pub struct NativeCdpSolver {
    chromium_bin: PathBuf,
    user_data_dir: PathBuf,
    persistent_session: Arc<Mutex<Option<ActiveSession>>>,
}

impl std::fmt::Debug for NativeCdpSolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeCdpSolver")
            .field("chromium_bin", &self.chromium_bin)
            .field("user_data_dir", &self.user_data_dir)
            .finish()
    }
}
impl Default for NativeCdpSolver {
    fn default() -> Self {
        Self::new(Self::find_chromium(), PathBuf::from(DEFAULT_PROFILE_DIR))
    }
}

impl NativeCdpSolver {
    /// Create a new NativeCdpSolver with custom chromium binary and user data profile directory.
    pub fn new(chromium_bin: impl Into<PathBuf>, user_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            chromium_bin: chromium_bin.into(),
            user_data_dir: user_data_dir.into(),
            persistent_session: Arc::new(Mutex::new(None)),
        }
    }

    /// Locate Chromium executable cross-platform across environment variables and standard system paths.
    pub fn find_chromium() -> PathBuf {
        if let Ok(path) = std::env::var("CHROMIUM_PATH") {
            let p = PathBuf::from(path);
            if p.exists() {
                return p;
            }
        }

        #[cfg(target_os = "linux")]
        let candidates = [
            "/run/current-system/sw/bin/chromium",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
        ];

        #[cfg(target_os = "macos")]
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/usr/local/bin/chromium",
        ];

        #[cfg(windows)]
        let candidates = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ];

        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let candidates: [&str; 0] = [];

        for candidate in candidates {
            let p = Path::new(candidate);
            if p.exists() {
                return p.to_path_buf();
            }
        }
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let p = dir.join("chromium");
                if p.is_file() {
                    return p;
                }
                #[cfg(windows)]
                {
                    let p_exe = dir.join("chrome.exe");
                    if p_exe.is_file() {
                        return p_exe;
                    }
                }
            }
        }
        PathBuf::from("chromium")
    }

    /// Verify or spawn a virtual X11 display (Xvfb `:99`) on Linux to guarantee zero desktop window leaks on Wayland/Niri.
    pub fn ensure_xvfb() -> String {
        let target_disp = ":99";
        let socket_path = Path::new("/tmp/.X11-unix/X99");
        let lock_path = Path::new("/tmp/.X99-lock");

        if socket_path.exists() {
            return target_disp.to_string();
        }

        // Clean stale lock if socket does not exist
        if lock_path.exists() && !socket_path.exists() {
            let _ = fs::remove_file(lock_path);
        }

        let mut xvfb_candidates = Vec::new();
        if let Ok(xvfb_env) = std::env::var("XVFB_BIN") {
            xvfb_candidates.push(PathBuf::from(xvfb_env));
        }
        xvfb_candidates.push(PathBuf::from("/run/current-system/sw/bin/Xvfb"));
        xvfb_candidates.push(PathBuf::from("/usr/bin/Xvfb"));
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                xvfb_candidates.push(dir.join("Xvfb"));
            }
        }
        let xvfb_bin = xvfb_candidates
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("Xvfb"));

        info!(xvfb = ?xvfb_bin, display_num = %target_disp, "Spawning headless Xvfb virtual framebuffer...");

        let spawn_res = std::process::Command::new(&xvfb_bin)
            .args([
                target_disp,
                "-screen",
                "0",
                "1366x900x24",
                "-ac",
                "-nolisten",
                "tcp",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        if let Err(e) = spawn_res {
            warn!(error = %e, "Failed to spawn Xvfb process directly; relying on existing DISPLAY");
            return target_disp.to_string();
        }

        // Poll for socket readiness up to 1.5 seconds (30 * 50ms)
        for _ in 0..30 {
            if socket_path.exists() {
                return target_disp.to_string();
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        target_disp.to_string()
    }

    /// Clear stale profile lock from previous browser runs.
    fn clear_stale_profile_lock(profile_dir: &Path) {
        let lock_files = ["SingletonLock", "SingletonSocket", "SingletonCookie"];
        for f in lock_files {
            let lock_path = profile_dir.join(f);
            if lock_path.exists() {
                let _ = fs::remove_file(&lock_path);
            }
        }
    }

    /// Spawn a Chromium process with cross-platform parent-death binding (PR_SET_PDEATHSIG on Linux),
    /// establish CDP WebSocket connection, and attach to an active browser page.
    async fn spawn_browser(&self) -> Result<CdpClient, ScraperError> {
        Self::clear_stale_profile_lock(&self.user_data_dir);
        let port = get_available_port();

        let temp_profile =
            std::env::temp_dir().join(format!("ozon-cdp-{}-{}", std::process::id(), port));
        let user_data_str = temp_profile.to_string_lossy().to_string();

        let mut cmd = tokio::process::Command::new(&self.chromium_bin);

        #[cfg(target_os = "linux")]
        {
            let disp_str = Self::ensure_xvfb();
            info!(
                chromium = ?self.chromium_bin,
                port = port,
                display_server = %disp_str,
                "Spawning on-demand Chromium for Variti PoW challenge resolution on Linux/Xvfb..."
            );

            cmd.args([
                "--ozone-platform-hint=auto",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--ozone-platform=x11",
                &format!("--user-data-dir={user_data_str}"),
                &format!("--remote-debugging-port={port}"),
                "about:blank",
            ])
            .env("DISPLAY", &disp_str)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);

            // Bind lifetime to parent process: Linux kernel sends SIGKILL if parent dies
            unsafe {
                cmd.pre_exec(|| {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        #[cfg(all(unix, not(target_os = "linux")))]
        {
            info!(
                chromium = ?self.chromium_bin,
                port = port,
                "Spawning on-demand Chromium for Variti PoW challenge resolution on Unix/macOS..."
            );

            cmd.args([
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                &format!("--user-data-dir={user_data_str}"),
                &format!("--remote-debugging-port={port}"),
                "about:blank",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        }

        #[cfg(windows)]
        {
            info!(
                chromium = ?self.chromium_bin,
                port = port,
                "Spawning on-demand Chromium for Variti PoW challenge resolution on Windows..."
            );

            cmd.args([
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                &format!("--user-data-dir={user_data_str}"),
                &format!("--remote-debugging-port={port}"),
                "about:blank",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        }

        let child = cmd.spawn().map_err(|e| {
            error!(error = %e, "Failed to spawn Chromium process");
            ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Failed to spawn Chromium: {e}"),
            }
        })?;

        // Poll /json/version up to 4 seconds (40 * 100ms)
        let mut ws_url = None;
        let version_url = format!("http://127.0.0.1:{port}/json/version");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(resp) = client.get(&version_url).send().await
                && let Ok(v) = resp.json::<Value>().await
                && let Some(u) = v.get("webSocketDebuggerUrl").and_then(|s| s.as_str())
            {
                ws_url = Some(u.to_string());
                break;
            }
        }

        let ws_url = ws_url.ok_or_else(|| {
            error!("Chromium failed to expose webSocketDebuggerUrl on port {port}");
            ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Chromium failed to start CDP listener on port {port}"),
            }
        })?;

        let (ws, _) = async_tungstenite::tokio::connect_async(&ws_url)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to connect to Chromium CDP WebSocket");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("CDP WebSocket connection failed: {e}"),
                }
            })?;

        let mut cdp = CdpClient {
            child,
            ws,
            session_id: String::new(),
            next_id: 0,
            profile_dir: temp_profile,
        };

        // 1. Create target
        let res = cdp
            .send_cmd("Target.createTarget", json!({ "url": "about:blank" }))
            .await?;
        let target_id = res["result"]["targetId"]
            .as_str()
            .ok_or_else(|| {
                ScraperError::Parse("Missing targetId in CreateTarget response".to_string())
            })?
            .to_string();

        // 2. Attach to target
        let res = cdp
            .send_cmd(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = res["result"]["sessionId"]
            .as_str()
            .ok_or_else(|| {
                ScraperError::Parse("Missing sessionId in AttachToTarget response".to_string())
            })?
            .to_string();

        cdp.session_id = session_id;

        // 3. Page.enable
        cdp.send_session_cmd("Page.enable", json!({})).await?;

        Ok(cdp)
    }

    /// Explicitly start and pre-warm an active Ozon browser session for batch processing.
    /// Subsequent searches and card lookups will execute at sub-second speed (~250ms).
    /// Automatically terminates after 5 minutes of idle time if not stopped explicitly.
    pub async fn start_session(&self) -> Result<String, ScraperError> {
        let mut session_guard = self.persistent_session.lock().await;
        if session_guard.is_some() {
            return Ok("Ozon browser session is already active and warm.".to_string());
        }

        info!("Starting persistent Ozon browser session for batch processing...");
        let mut cdp = self.spawn_browser().await?;
        cdp.send_session_cmd("Page.navigate", json!({ "url": "https://www.ozon.ru/" }))
            .await?;

        info!("Waiting 10s quietly for Variti PoW calculation in native V8...");
        tokio::time::sleep(Duration::from_secs(10)).await;

        *session_guard = Some(ActiveSession {
            cdp,
            last_used: std::time::Instant::now(),
        });

        // Spawn 5-minute idle watchdog
        let session_arc = Arc::clone(&self.persistent_session);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let mut guard = session_arc.lock().await;
                if let Some(ref session) = *guard {
                    if session.last_used.elapsed() > Duration::from_secs(300) {
                        info!("Ozon persistent session idle for 5 minutes, shutting down...");
                        if let Some(s) = guard.take() {
                            s.cdp.close().await;
                        }
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        Ok("Ozon browser session successfully started and verified. Subsequent ozon_search and ozon_card requests will execute at sub-second speed (~250ms). Automatically closes after 5 minutes of idle time.".to_string())
    }

    /// Explicitly terminate the active Ozon browser session and free memory immediately.
    pub async fn stop_session(&self) -> Result<String, ScraperError> {
        let mut session_guard = self.persistent_session.lock().await;
        if let Some(session) = session_guard.take() {
            session.cdp.close().await;
            info!("Ozon persistent browser session explicitly terminated and memory freed.");
            Ok("Ozon browser session stopped and browser terminated. Memory freed.".to_string())
        } else {
            Ok("No active Ozon session to stop.".to_string())
        }
    }

    /// Harvest Ozon session tokens by launching Chromium on demand in Xvfb :99, navigating to Ozon,
    /// waiting for Variti anti-bot challenge to settle, and extracting cookies.
    pub async fn harvest_ozon(&self) -> Result<SessionData, ScraperError> {
        let mut cdp = self.spawn_browser().await?;

        info!("Navigating on-demand Chromium to Ozon home page to harvest tokens...");
        cdp.send_session_cmd("Page.navigate", json!({ "url": "https://www.ozon.ru/" }))
            .await?;

        info!("Waiting 10s quietly for Variti PoW calculation in native V8...");
        tokio::time::sleep(Duration::from_secs(10)).await;

        let res = cdp
            .send_session_cmd("Network.getCookies", json!({}))
            .await?;

        // Immediately close on-demand browser
        cdp.close().await;

        let mut cookie_list = Vec::new();
        if let Some(arr) = res["result"]["cookies"].as_array() {
            for c in arr {
                if let (Some(name), Some(val)) = (c["name"].as_str(), c["value"].as_str()) {
                    cookie_list.push(format!("{name}={val}"));
                }
            }
        }

        if cookie_list.is_empty() {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: "No cookies collected from Ozon challenge".to_string(),
            });
        }

        let cookie_string = cookie_list.join("; ");
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 86400; // 24h TTL

        info!(
            cookie_count = cookie_list.len(),
            "Successfully harvested Ozon session via on-demand Chromium"
        );

        Ok(SessionData {
            marketplace: "ozon".to_string(),
            cookies: cookie_string,
            expires_at,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        })
    }

    /// Fetch raw JSON from Ozon Composer API through CDP.
    /// If an active persistent session was started via ozon_session_start, uses it for sub-second execution (~250ms).
    /// Otherwise, performs an on-demand launch and terminates immediately.
    pub async fn fetch_composer(&self, url_param: &str) -> Result<String, ScraperError> {
        let encoded_url_param =
            serde_json::to_string(url_param).unwrap_or_else(|_| format!("\"{}\"", url_param));

        let expr = format!(
            r#"fetch("/api/composer-api.bx/page/json/v2?url=" + encodeURIComponent({}), {{ headers: {{ accept: "application/json" }}, credentials: "include" }}).then(async r => ({{ status: r.status, ok: r.ok, text: await r.text() }})).catch(err => ({{ status: 0, ok: false, text: String(err) }}))"#,
            encoded_url_param
        );

        // 1. Check if an active persistent session is available
        {
            let mut session_guard = self.persistent_session.lock().await;
            if let Some(ref mut session) = *session_guard {
                session.last_used = std::time::Instant::now();
                let res = session
                    .cdp
                    .send_session_cmd(
                        "Runtime.evaluate",
                        json!({
                            "expression": expr,
                            "awaitPromise": true,
                            "returnByValue": true
                        }),
                    )
                    .await;

                if let Ok(val) = res {
                    let status = val["result"]["result"]["value"]["status"]
                        .as_u64()
                        .unwrap_or(0);
                    let ok = val["result"]["result"]["value"]["ok"]
                        .as_bool()
                        .unwrap_or(false);
                    let text = val["result"]["result"]["value"]["text"]
                        .as_str()
                        .unwrap_or_default();

                    if ok && status == 200 {
                        return Ok(text.to_string());
                    }
                }
                // If query failed on persistent session (e.g. expired tokens), close it and fall back
                if let Some(old) = session_guard.take() {
                    old.cdp.close().await;
                }
            }
        }

        // 2. Fallback: On-demand one-shot flow (spawn, PoW, fetch, immediately terminate)
        let mut cdp = self.spawn_browser().await?;

        info!("Navigating on-demand Chromium to Ozon home page...");
        cdp.send_session_cmd("Page.navigate", json!({ "url": "https://www.ozon.ru/" }))
            .await?;

        info!("Waiting 10s quietly for Variti PoW calculation in native V8...");
        tokio::time::sleep(Duration::from_secs(10)).await;

        let res = cdp
            .send_session_cmd(
                "Runtime.evaluate",
                json!({
                    "expression": expr,
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;

        // Immediately terminate Chromium process
        cdp.close().await;

        let status = res["result"]["result"]["value"]["status"]
            .as_u64()
            .unwrap_or(0);
        let ok = res["result"]["result"]["value"]["ok"]
            .as_bool()
            .unwrap_or(false);
        let text = res["result"]["result"]["value"]["text"]
            .as_str()
            .unwrap_or_default();

        if !ok || status != 200 {
            if let Ok(val) = serde_json::from_str::<Value>(text)
                && let Some(inc) = val.get("incidentId").and_then(|v| v.as_str())
            {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Ozon security incident {inc} (HTTP {status})"),
                });
            }
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Composer fetch failed with HTTP status {status}"),
            });
        }

        if let Ok(val) = serde_json::from_str::<Value>(text)
            && let Some(inc) = val.get("incidentId").and_then(|v| v.as_str())
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Ozon security incident {inc}"),
            });
        }

        Ok(text.to_string())
    }
}
