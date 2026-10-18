use ru_marketplace_solver::NativeCdpSolver;

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

#[test]
fn test_fetch_expression_headers() {
    use ru_marketplace_core::domain::Marketplace;

    let test_url =
        "https://www.wildberries.ru/__internal/u-search/exactmatch/ru/common/v18/search?query=test";
    let expr = NativeCdpSolver::build_fetch_expression(test_url);

    // Verify that the JS fetch expression correctly injects headers:
    // "x-requested-with", "XMLHttpRequest", "Accept-Language", "Accept", "credentials: \"include\""
    assert!(expr.contains("x-requested-with"));
    assert!(expr.contains("XMLHttpRequest"));
    assert!(expr.contains("Accept-Language"));
    assert!(expr.contains("ru-RU,ru;q=0.9"));
    assert!(expr.contains("Accept"));
    assert!(expr.contains("credentials: \"include\""));
    assert!(expr.contains("method: \"GET\""));

    // Verify origins
    assert_eq!(
        NativeCdpSolver::target_origin(Marketplace::Wildberries),
        "https://www.wildberries.ru"
    );
    assert_eq!(
        NativeCdpSolver::target_origin(Marketplace::Ozon),
        "https://www.ozon.ru"
    );
    assert_eq!(
        NativeCdpSolver::target_origin(Marketplace::Dns),
        "https://www.dns-shop.ru"
    );
    assert_eq!(
        NativeCdpSolver::target_origin(Marketplace::Citilink),
        "https://www.citilink.ru"
    );

    // Verify readiness check script
    let script = NativeCdpSolver::build_readiness_script("https://www.wildberries.ru");
    assert!(script.contains("window.location.origin !== \"https://www.wildberries.ru\""));
    assert!(script.contains("document.readyState === \"loading\""));
}

#[test]
fn test_stage_detection_script_and_page_stages() {
    use ru_marketplace_solver::PageStage;

    let script = NativeCdpSolver::build_stage_detection_script();
    assert!(script.contains("challenge_active"));
    assert!(script.contains("navigating"));
    assert!(script.contains("ready"));
    assert!(script.contains("blocked"));
    assert!(script.contains("qrator_js"));
    assert!(script.contains(".catalog-product"));
    assert!(script.contains("ProductVerticalSnippet"));

    assert_eq!(PageStage::Ready, PageStage::Ready);
    assert_ne!(PageStage::Ready, PageStage::ChallengeActive);
    assert_ne!(PageStage::Navigating, PageStage::Blocked);
}

#[test]
fn test_base64_roundtrip_and_edge_cases() {
    use ru_marketplace_solver::{decode_base64, encode_base64};

    // Empty
    assert_eq!(encode_base64(b""), "");
    assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());

    // 1-byte
    assert_eq!(encode_base64(b"f"), "Zg==");
    assert_eq!(decode_base64("Zg==").unwrap(), b"f");

    // 2-bytes
    assert_eq!(encode_base64(b"fo"), "Zm8=");
    assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");

    // 3-bytes
    assert_eq!(encode_base64(b"foo"), "Zm9v");
    assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");

    // JPEG header simulation
    let jpeg_sample = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
    let encoded = encode_base64(&jpeg_sample);
    let decoded = decode_base64(&encoded).unwrap();
    assert_eq!(decoded, jpeg_sample);

    // Whitespace toleration
    let with_whitespace = format!("  \n{}\r\n  ", encoded);
    assert_eq!(decode_base64(&with_whitespace).unwrap(), jpeg_sample);

    // Errors
    assert!(decode_base64("not_base64!").is_err());
    assert!(decode_base64("abc").is_err());
}

#[test]
fn test_solver_error_variants() {
    use ru_marketplace_core::domain::ScraperError;
    use ru_marketplace_solver::SolverError;

    let err1 = SolverError::Blocked("access denied".to_string());
    assert!(err1.to_string().contains("access denied"));

    let err2 = SolverError::CaptureFailed("no data".to_string());
    assert!(err2.to_string().contains("no data"));

    let scraper_err = ScraperError::Timeout;
    let err3 = SolverError::from(scraper_err);
    assert!(err3.to_string().contains("Request timed out"));
}
