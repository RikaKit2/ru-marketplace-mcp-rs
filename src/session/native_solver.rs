use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures::StreamExt;
use serde_json::{Value, json};
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
        let _ = tokio::time::sleep(Duration::from_millis(150)).await;

        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        let _ = self.child.kill().await;
        let _ = std::fs::remove_dir_all(&self.profile_dir);
    }
}

/// Pure Rust CDP solver that spawns Chromium on-demand in Xvfb :99 to solve
/// Variti Proof-of-Work and execute authenticated queries with 100% genuine Chrome TLS.
#[derive(Debug, Clone)]
pub struct NativeCdpSolver {
    chromium_bin: PathBuf,
    user_data_dir: PathBuf,
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
        }
    }

    /// Locate Chromium executable across environment variables and standard system paths.
    pub fn find_chromium() -> PathBuf {
        if let Ok(path) = std::env::var("CHROMIUM_PATH") {
            let p = PathBuf::from(path);
            if p.exists() {
                return p;
            }
        }

        let candidates = [
            "/run/current-system/sw/bin/chromium",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
        ];

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
            }
        }
        PathBuf::from("chromium")
    }

    /// If path is a NixOS wrapper script, extract the real underlying Chromium binary
    /// to bypass environment overrides (such as forced Wayland ozone hints).
    pub fn resolve_unwrapped_binary(path: &Path) -> PathBuf {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("exec \"")
                    && let Some(bin_path) = rest.split('"').next()
                {
                    let unwrapped = PathBuf::from(bin_path);
                    if unwrapped.is_file() {
                        return unwrapped;
                    }
                }
            }
        }
        path.to_path_buf()
    }

    /// Verify or spawn a virtual X11 display (Xvfb `:99`) to guarantee zero desktop window leaks on Wayland/Niri.
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

    /// Spawn an on-demand Chromium process in Xvfb :99, establish CDP WebSocket connection,
    /// and attach to an active browser page.
    async fn spawn_browser(&self) -> Result<CdpClient, ScraperError> {
        let disp_str = Self::ensure_xvfb();
        Self::clear_stale_profile_lock(&self.user_data_dir);
        let port = get_available_port();

        info!(
            chromium = ?self.chromium_bin,
            port = port,
            display_server = %disp_str,
            "Spawning on-demand Chromium for Variti PoW challenge resolution..."
        );

        let temp_profile =
            std::env::temp_dir().join(format!("ozon-cdp-{}-{}", std::process::id(), port));
        let user_data_str = temp_profile.to_string_lossy().to_string();

        let mut cmd = tokio::process::Command::new(&self.chromium_bin);
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

        // Guarantee immediate process termination (Способ 2)
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

    /// Fetch raw JSON from Ozon Composer API through on-demand Chromium CDP evaluation.
    pub async fn fetch_composer(&self, url_param: &str) -> Result<String, ScraperError> {
        let mut cdp = self.spawn_browser().await?;
        info!("Navigating on-demand Chromium to Ozon home page...");
        cdp.send_session_cmd("Page.navigate", json!({ "url": "https://www.ozon.ru/" }))
            .await?;

        info!("Waiting 10s quietly for Variti PoW calculation in native V8...");
        tokio::time::sleep(Duration::from_secs(10)).await;

        let encoded_url_param =
            serde_json::to_string(url_param).unwrap_or_else(|_| format!("\"{}\"", url_param));

        let expr = format!(
            r#"fetch("/api/composer-api.bx/page/json/v2?url=" + encodeURIComponent({}), {{ headers: {{ accept: "application/json" }}, credentials: "include" }}).then(async r => ({{ status: r.status, ok: r.ok, text: await r.text() }})).catch(err => ({{ status: 0, ok: false, text: String(err) }}))"#,
            encoded_url_param
        );

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

        // Guarantee immediate process termination (Способ 2)
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
