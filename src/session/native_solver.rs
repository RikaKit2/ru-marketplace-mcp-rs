use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use super::cookies::SessionData;
use crate::domain::{ScraperError, WafProtection};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";
const DEFAULT_PROFILE_DIR: &str = "/tmp/ozon-native-cdp-profile";

/// Pure Rust CDP solver using `chromiumoxide` to solve anti-bot challenges
/// (Variti / Cloudflare / WAF) and harvest marketplace credentials in an isolated Xvfb framebuffer.
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

    /// Harvest Ozon session tokens by launching Chromium in headless Xvfb, navigating to Ozon,
    /// waiting for Variti anti-bot challenge to settle, and extracting cookies.
    pub async fn harvest_ozon(&self) -> Result<SessionData, ScraperError> {
        info!(
            chromium = ?self.chromium_bin,
            profile = ?self.user_data_dir,
            "Harvesting Ozon session via NativeCdpSolver (chromiumoxide)..."
        );

        // Strict isolation: strip Wayland display so Chromium NEVER connects to Niri
        // In Rust 2024 edition, environment variable mutations are marked unsafe.
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let display = Self::ensure_xvfb();
        unsafe {
            std::env::set_var("DISPLAY", &display);
        }
        Self::clear_stale_profile_lock(&self.user_data_dir);

        let user_data_str = self.user_data_dir.to_string_lossy().to_string();

        let config = BrowserConfig::builder()
            .chrome_executable(&self.chromium_bin)
            .new_headless_mode()
            .no_sandbox()
            .user_data_dir(&self.user_data_dir)
            .env("DISPLAY", &display)
            .env("WAYLAND_DISPLAY", "")
            .args([
                "--disable-blink-features=AutomationControlled",
                "--no-sandbox",
                "--disable-setuid-sandbox",
                "--disable-dev-shm-usage",
                "--disable-gpu",
                "--ozone-platform=x11",
                &format!("--user-data-dir={user_data_str}"),
            ])
            .build()
            .map_err(|e| {
                error!(error = %e, "Failed to build BrowserConfig for chromiumoxide");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Failed to configure Chromium: {e}"),
                }
            })?;

        let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            error!(error = %e, "Failed to launch Chromium browser via chromiumoxide");
            ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Failed to launch Chromium: {e}"),
            }
        })?;

        // Spawn handler task driving WebSocket event loop
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    debug!(error = %e, "CDP handler event error");
                    break;
                }
            }
        });

        let harvest_result = self.drive_ozon_harvest(&mut browser).await;

        // Ensure browser closes gracefully or gets killed
        if let Err(e) = browser.close().await {
            debug!(error = %e, "Browser close error or already closed");
        }
        handler_task.abort();

        harvest_result
    }
    /// Fetch raw JSON from Ozon Composer API through CDP in-memory page evaluation.
    pub async fn fetch_composer(&self, url_param: &str) -> Result<String, ScraperError> {
        info!(
            url_param = %url_param,
            chromium = ?self.chromium_bin,
            profile = ?self.user_data_dir,
            "Fetching Ozon Composer JSON via NativeCdpSolver (in-memory CDP)..."
        );

        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let display = Self::ensure_xvfb();
        unsafe {
            std::env::set_var("DISPLAY", &display);
        }
        Self::clear_stale_profile_lock(&self.user_data_dir);

        let user_data_str = self.user_data_dir.to_string_lossy().to_string();

        let config = BrowserConfig::builder()
            .chrome_executable(&self.chromium_bin)
            .new_headless_mode()
            .user_data_dir(&self.user_data_dir)
            .env("DISPLAY", &display)
            .env("WAYLAND_DISPLAY", "")
            .args([
                "--disable-blink-features=AutomationControlled",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--ozone-platform=x11",
                &format!("--user-data-dir={user_data_str}"),
            ])
            .build()
            .map_err(|e| {
                error!(error = %e, "Failed to build BrowserConfig for chromiumoxide");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Failed to configure Chromium: {e}"),
                }
            })?;

        let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            error!(error = %e, "Failed to launch Chromium browser via chromiumoxide");
            ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Failed to launch Chromium: {e}"),
            }
        })?;

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    debug!(error = %e, "CDP handler event error");
                    break;
                }
            }
        });

        let fetch_result = self.drive_composer_fetch(&mut browser, url_param).await;

        if let Err(e) = browser.close().await {
            debug!(error = %e, "Browser close error or already closed");
        }
        handler_task.abort();

        fetch_result
    }

    async fn drive_composer_fetch(
        &self,
        browser: &mut Browser,
        url_param: &str,
    ) -> Result<String, ScraperError> {
        let page = browser
            .new_page("https://www.ozon.ru/")
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to open page https://www.ozon.ru/ for composer fetch");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Failed to open Ozon page: {e}"),
                }
            })?;

        // Wait for Variti challenge to settle and session cookies to be present
        tokio::time::sleep(Duration::from_secs(4)).await;

        let encoded_url_param =
            serde_json::to_string(url_param).unwrap_or_else(|_| format!("\"{}\"", url_param));
        let script = format!(
            "() => fetch('/api/composer-api.bx/page/json/v2?url=' + encodeURIComponent({}), {{ headers: {{ accept: 'application/json' }}, credentials: 'include' }}).then(r => r.text())",
            encoded_url_param
        );

        let json_result: String = page
            .evaluate_function(script)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to evaluate fetch_composer expression via CDP");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Failed to evaluate fetch_composer script: {e}"),
                }
            })?
            .into_value()
            .map_err(|e| {
                error!(error = %e, "Failed to deserialize fetch_composer result string");
                ScraperError::Parse(format!("Failed to parse fetch_composer string result: {e}"))
            })?;

        Ok(json_result)
    }

    async fn drive_ozon_harvest(&self, browser: &mut Browser) -> Result<SessionData, ScraperError> {
        let page = browser
            .new_page("https://www.ozon.ru/")
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to open page https://www.ozon.ru/");
                ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Failed to open Ozon page: {e}"),
                }
            })?;

        // Wait for Variti challenge to settle and set cookies
        info!("Waiting 6s for Ozon Variti challenge settlement...");
        tokio::time::sleep(Duration::from_secs(6)).await;

        let cookies = page.get_cookies().await.map_err(|e| {
            error!(error = %e, "Failed to extract cookies via CDP page.get_cookies()");
            ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Failed to extract cookies: {e}"),
            }
        })?;

        let cookie_list: Vec<String> = cookies
            .into_iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();

        if cookie_list.is_empty() {
            error!("Zero cookies retrieved from Ozon challenge page");
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
            "Successfully harvested Ozon session via NativeCdpSolver"
        );

        Ok(SessionData {
            marketplace: "ozon".to_string(),
            cookies: cookie_string,
            expires_at,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        })
    }
}
