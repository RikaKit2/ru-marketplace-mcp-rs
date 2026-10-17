use ru_marketplace_mcp::session::NativeCdpSolver;

#[test]
fn test_find_chromium_locates_binary_or_fallback() {
    let chromium_path = NativeCdpSolver::find_chromium();
    assert!(!chromium_path.as_os_str().is_empty());
    assert!(chromium_path.exists());
}

#[test]
fn test_ensure_xvfb_returns_display_99() {
    let display = NativeCdpSolver::ensure_xvfb();
    assert_eq!(display, ":99");
}

#[tokio::test]
async fn test_native_cdp_evaluates_simple_script() {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures::StreamExt;

    let chromium_bin = NativeCdpSolver::find_chromium();
    if !chromium_bin.exists() {
        eprintln!(
            "Chromium binary not found at {:?}, skipping test",
            chromium_bin
        );
        return;
    }

    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    let display = NativeCdpSolver::ensure_xvfb();
    unsafe {
        std::env::set_var("DISPLAY", &display);
    }

    let temp_profile =
        std::env::temp_dir().join(format!("test-cdp-profile-{}", std::process::id()));
    let temp_profile_str = temp_profile.to_string_lossy().to_string();

    let config = BrowserConfig::builder()
        .chrome_executable(&chromium_bin)
        .new_headless_mode()
        .no_sandbox()
        .user_data_dir(&temp_profile)
        .env("DISPLAY", &display)
        .env("WAYLAND_DISPLAY", "")
        .args([
            "--disable-blink-features=AutomationControlled",
            "--no-sandbox",
            "--disable-setuid-sandbox",
            "--disable-dev-shm-usage",
            "--disable-gpu",
            "--ozone-platform=x11",
            &format!("--user-data-dir={temp_profile_str}"),
        ])
        .build()
        .expect("build BrowserConfig");

    let (mut browser, mut handler) = match Browser::launch(config).await {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Browser::launch failed in this environment ({e:?}), skipping test");
            let _ = std::fs::remove_dir_all(&temp_profile);
            return;
        }
    };

    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let page = browser.new_page("about:blank").await.expect("new page");
    let sum: usize = page
        .evaluate_function("() => Promise.resolve(21 * 2)")
        .await
        .expect("evaluate function")
        .into_value()
        .expect("into value");

    assert_eq!(sum, 42);

    let _ = browser.close().await;
    handler_task.abort();
    let _ = std::fs::remove_dir_all(&temp_profile);
}
