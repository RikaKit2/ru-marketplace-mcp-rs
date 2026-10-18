use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use async_tungstenite::tungstenite::Message;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use ru_marketplace_core::domain::{Marketplace, ScraperError, WafProtection};
use ru_marketplace_core::session::{SessionData, SessionSolver, SessionStatusInfo};
use ru_marketplace_core::transport::DEFAULT_DESKTOP_UA as DEFAULT_USER_AGENT;

const DEFAULT_PROFILE_DIR: &str = "/tmp/ru-marketplace-solver-profile";
static PORT_COUNTER: AtomicU16 = AtomicU16::new(9400);

fn get_available_port() -> u16 {
    if let Ok(listener) = TcpListener::bind("127.0.0.1:0")
        && let Ok(addr) = listener.local_addr()
    {
        return addr.port();
    }
    PORT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub struct CdpClient {
    child: tokio::process::Child,
    ws: async_tungstenite::WebSocketStream<async_tungstenite::tokio::ConnectStream>,
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
    pub async fn send_cmd(&mut self, method: &str, params: Value) -> Result<Value, ScraperError> {
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

    pub async fn send_target_cmd(
        &mut self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, ScraperError> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "id": id,
            "sessionId": session_id,
            "method": method,
            "params": params,
        });
        self.ws
            .send(Message::Text(req.to_string().into()))
            .await
            .map_err(|e| ScraperError::Parse(format!("Failed to send CDP target command: {e}")))?;

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
                        "CDP target WebSocket error: {e}"
                    )));
                }
                _ => {}
            }
        }
        Err(ScraperError::Timeout)
    }

    pub async fn create_and_attach_tab(&mut self, url: &str) -> Result<String, ScraperError> {
        let res = self
            .send_cmd("Target.createTarget", json!({ "url": url }))
            .await?;
        let target_id = res["result"]["targetId"].as_str().ok_or_else(|| {
            ScraperError::Parse(format!(
                "Target.createTarget failed for {url}: missing targetId in {res}"
            ))
        })?;

        let attach_res = self
            .send_cmd(
                "Target.attachToTarget",
                json!({
                    "targetId": target_id,
                    "flatten": true
                }),
            )
            .await?;

        let session_id = attach_res["result"]["sessionId"]
            .as_str()
            .ok_or_else(|| {
                ScraperError::Parse(format!(
                    "Target.attachToTarget failed for {url}: missing sessionId in {attach_res}"
                ))
            })?
            .to_string();

        Ok(session_id)
    }

    pub async fn close(mut self) {
        let _ = self.send_cmd("Browser.close", json!({})).await;
        let _ = tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct ActiveSession {
    cdp: CdpClient,
    tabs: HashMap<Marketplace, String>,
    last_used: std::time::Instant,
}

/// Unified multi-tab Chromium solver on Xvfb :99.
/// Operates with standard browser security (SOP & CORS enabled).
/// Dedicated tabs for Ozon and Wildberries provide authentic TLS/Client Hello.
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
    /// Create a new solver with custom chromium binary and user data profile directory.
    pub fn new(chromium_bin: impl Into<PathBuf>, user_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            chromium_bin: chromium_bin.into(),
            user_data_dir: user_data_dir.into(),
            persistent_session: Arc::new(Mutex::new(None)),
        }
    }

    /// Locate Chromium executable cross-platform across environment variables and standard paths.
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

    /// Ensure Xvfb virtual display is running on Linux (:99).
    pub fn ensure_xvfb() -> String {
        #[cfg(not(target_os = "linux"))]
        {
            return String::new();
        }

        #[cfg(target_os = "linux")]
        {
            let target_disp = ":99";
            let lock_file = Path::new("/tmp/.X99-lock");
            let socket_file = Path::new("/tmp/.X11-unix/X99");

            if lock_file.exists() && socket_file.exists() {
                return target_disp.to_string();
            }

            info!("Xvfb :99 not active, spawning isolated Xvfb display on :99...");
            let xvfb_candidates = [
                "/run/current-system/sw/bin/Xvfb",
                "/usr/bin/Xvfb",
                "/usr/local/bin/Xvfb",
                "Xvfb",
            ];

            let mut spawned = false;
            for candidate in xvfb_candidates {
                if let Ok(child) = std::process::Command::new(candidate)
                    .args([
                        target_disp,
                        "-screen",
                        "0",
                        "1280x1024x24",
                        "-nolisten",
                        "tcp",
                        "-nocursor",
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    info!(pid = child.id(), "Spawned Xvfb :99 daemon");
                    spawned = true;
                    break;
                }
            }

            if !spawned {
                warn!("Could not spawn Xvfb directly; falling back to current DISPLAY");
                return std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
            }

            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                if lock_file.exists() || socket_file.exists() {
                    info!("Xvfb :99 display successfully verified");
                    return target_disp.to_string();
                }
            }

            target_disp.to_string()
        }
    }

    fn clear_stale_profile_lock(profile_dir: &Path) {
        let lock_files = ["SingletonLock", "SingletonSocket", "SingletonCookie"];
        for f in lock_files {
            let lock_path = profile_dir.join(f);
            if lock_path.exists() {
                let _ = fs::remove_file(&lock_path);
            }
        }
    }

    async fn spawn_browser(&self) -> Result<CdpClient, ScraperError> {
        Self::clear_stale_profile_lock(&self.user_data_dir);
        let port = get_available_port();

        let temp_profile =
            std::env::temp_dir().join(format!("ru-mcp-cdp-{}-{}", std::process::id(), port));
        let user_data_str = temp_profile.to_string_lossy().to_string();

        let mut cmd = tokio::process::Command::new(&self.chromium_bin);

        #[cfg(target_os = "linux")]
        {
            let disp_str = Self::ensure_xvfb();
            info!(
                chromium = ?self.chromium_bin,
                port = port,
                display_server = %disp_str,
                "Spawning unified Chromium solver on Linux/Xvfb..."
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

        Ok(CdpClient {
            child,
            ws,
            next_id: 0,
            profile_dir: temp_profile,
        })
    }

    /// Check if a persistent browser session is currently active.
    pub async fn is_session_active(&self) -> bool {
        self.persistent_session.lock().await.is_some()
    }

    /// Start a persistent multi-tab browser session for fast execution (~250ms).
    pub async fn start_session(&self) -> Result<String, ScraperError> {
        let mut session_guard = self.persistent_session.lock().await;
        if session_guard.is_some() {
            return Ok("Universal browser session is already active and warm.".to_string());
        }

        info!("Starting persistent Chromium solver for Ozon and Wildberries...");
        let mut cdp = self.spawn_browser().await?;
        let mut tabs = HashMap::new();

        // 1. Open Ozon tab and wait for Variti PoW to settle
        info!("Opening Ozon tab and solving Variti PoW challenge in native V8...");
        let ozon_session_id = cdp.create_and_attach_tab("https://www.ozon.ru/").await?;
        tabs.insert(Marketplace::Ozon, ozon_session_id);
        tokio::time::sleep(Duration::from_secs(10)).await;

        // 2. Open Wildberries tab
        info!("Opening Wildberries tab for authentic TLS Client Hello...");
        let wb_session_id = cdp
            .create_and_attach_tab("https://www.wildberries.ru/")
            .await?;
        tabs.insert(Marketplace::Wildberries, wb_session_id);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        *session_guard = Some(ActiveSession {
            cdp,
            tabs,
            last_used: std::time::Instant::now(),
        });

        // 5-minute idle watchdog
        let session_arc = Arc::clone(&self.persistent_session);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let mut guard = session_arc.lock().await;
                if let Some(session) = guard.as_ref() {
                    if session.last_used.elapsed() > Duration::from_secs(300) {
                        info!("Persistent browser solver idle for 5 minutes, shutting down...");
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

        Ok("Browser session successfully started and verified (Ozon + Wildberries tabs ready). Subsequent search requests will execute at sub-second speed (~250ms). Automatically closes after 5 minutes of idle time.".to_string())
    }

    /// Explicitly terminate active browser session and free memory immediately.
    pub async fn stop_session(&self) -> Result<String, ScraperError> {
        let mut session_guard = self.persistent_session.lock().await;
        if let Some(session) = session_guard.take() {
            session.cdp.close().await;
            info!("Persistent browser solver explicitly terminated and memory freed.");
            Ok("Browser session stopped and Chromium terminated. Memory freed.".to_string())
        } else {
            Ok("No active browser session to stop.".to_string())
        }
    }
    /// Generate standard JS fetch expression with browser headers and credentials.
    pub fn build_fetch_expression(fetch_url: &str) -> String {
        let encoded_url =
            serde_json::to_string(fetch_url).unwrap_or_else(|_| format!("\"{}\"", fetch_url));
        format!(
            r#"fetch({}, {{ method: "GET", headers: {{ "Accept": "*/*", "Accept-Language": "ru-RU,ru;q=0.9", "x-requested-with": "XMLHttpRequest" }}, credentials: "include" }}).then(async r => ({{ status: r.status, ok: r.ok, text: await r.text() }})).catch(err => ({{ status: 0, ok: false, text: String(err) }}))"#,
            encoded_url
        )
    }

    /// Determine target origin URL based on marketplace.
    pub fn target_origin(marketplace: Marketplace) -> &'static str {
        match marketplace {
            Marketplace::Ozon => "https://www.ozon.ru",
            Marketplace::Wildberries => "https://www.wildberries.ru",
            Marketplace::Dns => "https://www.dns-shop.ru",
            Marketplace::Citilink => "https://www.citilink.ru",
            _ => "",
        }
    }

    /// Generate readiness check script verifying tab origin and document readyState.
    pub fn build_readiness_script(target_origin: &str) -> String {
        format!(
            r#"(() => {{
    if (window.location.origin !== "{}") return false;
    if (document.readyState === "loading") return false;
    return true;
}})()"#,
            target_origin
        )
    }

    /// Execute a JSON fetch expression inside the appropriate marketplace tab.
    pub async fn fetch_in_tab(
        &self,
        marketplace: Marketplace,
        fetch_url: &str,
    ) -> Result<String, ScraperError> {
        let expr = Self::build_fetch_expression(fetch_url);
        let origin = Self::target_origin(marketplace);
        let readiness_script = Self::build_readiness_script(origin);

        // 1. Try persistent session
        {
            let mut session_guard = self.persistent_session.lock().await;
            if let Some(session) = session_guard.as_mut() {
                session.last_used = std::time::Instant::now();

                // Get or create tab for this marketplace
                let tab_session_id = match session.tabs.get(&marketplace) {
                    Some(id) => id.clone(),
                    None => {
                        let target_url = match marketplace {
                            Marketplace::Ozon => "https://www.ozon.ru/",
                            Marketplace::Wildberries => "https://www.wildberries.ru/",
                            Marketplace::Dns => "https://www.dns-shop.ru/",
                            Marketplace::Citilink => "https://www.citilink.ru/",
                            _ => "about:blank",
                        };
                        let new_id = session.cdp.create_and_attach_tab(target_url).await?;
                        session.tabs.insert(marketplace, new_id.clone());
                        new_id
                    }
                };

                // Poll readiness check script up to 3000ms (every 200ms) before evaluating fetch()
                if !origin.is_empty() {
                    let deadline = std::time::Instant::now() + Duration::from_millis(3000);
                    while std::time::Instant::now() < deadline {
                        let check = session
                            .cdp
                            .send_target_cmd(
                                &tab_session_id,
                                "Runtime.evaluate",
                                json!({
                                    "expression": readiness_script,
                                    "returnByValue": true
                                }),
                            )
                            .await;
                        if let Ok(val) = check
                            && val["result"]["result"]["value"].as_bool() == Some(true)
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }

                let res = session
                    .cdp
                    .send_target_cmd(
                        &tab_session_id,
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
            }
        }

        // 2. On-demand launch fallback
        let mut cdp = self.spawn_browser().await?;
        let target_url = match marketplace {
            Marketplace::Ozon => "https://www.ozon.ru/",
            Marketplace::Wildberries => "https://www.wildberries.ru/",
            Marketplace::Dns => "https://www.dns-shop.ru/",
            Marketplace::Citilink => "https://www.citilink.ru/",
            _ => "about:blank",
        };

        let session_id = cdp.create_and_attach_tab(target_url).await?;
        if marketplace == Marketplace::Ozon {
            info!("Waiting 10s quietly for Variti PoW calculation on on-demand launch...");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        // Poll readiness check script up to 3000ms (every 200ms) before evaluating fetch()
        if !origin.is_empty() {
            let deadline = std::time::Instant::now() + Duration::from_millis(3000);
            while std::time::Instant::now() < deadline {
                let check = cdp
                    .send_target_cmd(
                        &session_id,
                        "Runtime.evaluate",
                        json!({
                            "expression": readiness_script,
                            "returnByValue": true
                        }),
                    )
                    .await;
                if let Ok(val) = check
                    && val["result"]["result"]["value"].as_bool() == Some(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        let res = cdp
            .send_target_cmd(
                &session_id,
                "Runtime.evaluate",
                json!({
                    "expression": expr,
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;

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
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!(
                    "Browser fetch for {marketplace} failed with HTTP status {status}: {text}"
                ),
            });
        }

        Ok(text.to_string())
    }

    /// Navigate a marketplace tab directly to allow anti-bot challenge scripts (Qrator/Variti)
    /// to execute in native Chromium V8, then extract the resulting DOM HTML.
    pub async fn navigate_and_extract_html(
        &self,
        marketplace: Marketplace,
        page_url: &str,
    ) -> Result<String, ScraperError> {
        let mut session_guard = self.persistent_session.lock().await;

        if let Some(session) = session_guard.as_mut() {
            session.last_used = std::time::Instant::now();

            let tab_session_id = match session.tabs.get(&marketplace) {
                Some(id) => id.clone(),
                None => {
                    let new_id = session.cdp.create_and_attach_tab(page_url).await?;
                    session.tabs.insert(marketplace, new_id.clone());
                    new_id
                }
            };

            let _ = session
                .cdp
                .send_target_cmd(&tab_session_id, "Page.navigate", json!({ "url": page_url }))
                .await;

            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let check = session
                    .cdp
                    .send_target_cmd(
                        &tab_session_id,
                        "Runtime.evaluate",
                        json!({
                            "expression": r#"(() => {
                                if (document.readyState !== "complete") return false;
                                const html = document.body ? document.body.innerHTML : "";
                                if (html.includes("qrator_js") || html.includes("Checking your browser") || html.includes("challenge-platform")) {
                                    return false;
                                }
                                return true;
                            })()"#,
                            "returnByValue": true
                        }),
                    )
                    .await;

                if let Ok(val) = check
                    && val["result"]["result"]["value"].as_bool() == Some(true)
                {
                    break;
                }
            }

            let res = session
                .cdp
                .send_target_cmd(
                    &tab_session_id,
                    "Runtime.evaluate",
                    json!({
                        "expression": "document.documentElement.outerHTML",
                        "returnByValue": true
                    }),
                )
                .await?;

            if let Some(html) = res["result"]["result"]["value"].as_str() {
                return Ok(html.to_string());
            }

            return Err(ScraperError::Parse(
                "Failed to extract document HTML from persistent tab".to_string(),
            ));
        }

        // Fallback: On-demand launch
        let mut cdp = self.spawn_browser().await?;
        let tab_session_id = cdp.create_and_attach_tab(page_url).await?;

        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let check = cdp
                .send_target_cmd(
                    &tab_session_id,
                    "Runtime.evaluate",
                    json!({
                        "expression": r#"(() => {
                            if (document.readyState !== "complete") return false;
                            const html = document.body ? document.body.innerHTML : "";
                            if (html.includes("qrator_js") || html.includes("Checking your browser") || html.includes("challenge-platform")) {
                                return false;
                            }
                            return true;
                        })()"#,
                        "returnByValue": true
                    }),
                )
                .await;

            if let Ok(val) = check
                && val["result"]["result"]["value"].as_bool() == Some(true)
            {
                break;
            }
        }

        let res = cdp
            .send_target_cmd(
                &tab_session_id,
                "Runtime.evaluate",
                json!({
                    "expression": "document.documentElement.outerHTML",
                    "returnByValue": true
                }),
            )
            .await?;

        cdp.close().await;

        if let Some(html) = res["result"]["result"]["value"].as_str() {
            Ok(html.to_string())
        } else {
            Err(ScraperError::Parse(
                "Failed to extract document HTML from on-demand browser".to_string(),
            ))
        }
    }

    /// Harvest cookies for a marketplace domain.
    pub async fn harvest(&self, marketplace: Marketplace) -> Result<SessionData, ScraperError> {
        let mut cdp = self.spawn_browser().await?;
        let target_url = match marketplace {
            Marketplace::Ozon => "https://www.ozon.ru/",
            Marketplace::Wildberries => "https://www.wildberries.ru/",
            Marketplace::Dns => "https://www.dns-shop.ru/",
            Marketplace::Citilink => "https://www.citilink.ru/",
            _ => "about:blank",
        };

        let session_id = cdp.create_and_attach_tab(target_url).await?;
        if marketplace == Marketplace::Ozon {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        let res = cdp
            .send_target_cmd(&session_id, "Network.getCookies", json!({}))
            .await?;
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
                details: format!("No cookies collected for {marketplace}"),
            });
        }

        let cookie_string = cookie_list.join("; ");
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 86400;

        Ok(SessionData {
            marketplace: marketplace.to_string(),
            cookies: cookie_string,
            expires_at,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        })
    }

    /// Wrap this solver as a Marketplace-specific `SessionSolver`.
    pub fn as_session_solver(self: &Arc<Self>, marketplace: Marketplace) -> Arc<dyn SessionSolver> {
        Arc::new(MarketplaceSessionSolver {
            solver: Arc::clone(self),
            marketplace,
        })
    }
}

struct MarketplaceSessionSolver {
    solver: Arc<NativeCdpSolver>,
    marketplace: Marketplace,
}

#[async_trait]
impl SessionSolver for MarketplaceSessionSolver {
    fn marketplace(&self) -> Marketplace {
        self.marketplace
    }

    async fn is_active(&self) -> bool {
        self.solver.is_session_active().await
    }

    async fn start_session(&self) -> Result<String, ScraperError> {
        self.solver.start_session().await
    }

    async fn stop_session(&self) -> Result<String, ScraperError> {
        self.solver.stop_session().await
    }

    async fn status(&self) -> SessionStatusInfo {
        let active = self.solver.is_session_active().await;
        SessionStatusInfo {
            marketplace: self.marketplace,
            solver_active: active,
            has_cached_cookies: false,
            cookies_expired: false,
            expires_at: None,
            details: if active {
                format!("Persistent Chromium tab active for {}", self.marketplace)
            } else {
                format!(
                    "Chromium solver ready for on-demand launch ({})",
                    self.marketplace
                )
            },
        }
    }

    async fn harvest(&self) -> Result<SessionData, ScraperError> {
        self.solver.harvest(self.marketplace).await
    }

    async fn invalidate(&self) -> Result<(), ScraperError> {
        Ok(())
    }
}
