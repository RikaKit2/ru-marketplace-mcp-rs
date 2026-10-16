use std::time::{SystemTime, UNIX_EPOCH};

use ru_marketplace_mcp::session::{CookieStore, SessionData, SessionManager};
use ru_marketplace_mcp::transport::{DEFAULT_DESKTOP_UA, SEC_CH_UA, build_browser_headers};

fn unique_temp_cache_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("ru-mcp-test-{name}-{nanos}"))
        .join("cookies.json")
}

#[tokio::test]
async fn test_session_data_expiration() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let expired_session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "test_cookie=1".to_string(),
        expires_at: now - 100,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(expired_session.is_expired());

    let expiring_soon_session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "test_cookie=2".to_string(),
        expires_at: now + 30, // within 60s safety buffer
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(expiring_soon_session.is_expired());

    let valid_session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "test_cookie=3".to_string(),
        expires_at: now + 86400,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(!valid_session.is_expired());
}

#[tokio::test]
async fn test_cookie_store_file_persistence() {
    let cache_path = unique_temp_cache_path("store-persistence");

    let store = CookieStore::new(&cache_path);

    // Initial get should be None
    assert!(store.get("wildberries").await.is_none());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "x_wbaas_token=token123".to_string(),
        expires_at: now + 3600,
        user_agent: "TestUA/1.0".to_string(),
    };

    store.insert(session.clone()).await.unwrap();

    // Verify it is in memory
    let fetched = store
        .get("wildberries")
        .await
        .expect("Session should exist");
    assert_eq!(fetched, session);

    // Reopen store from file path to verify persistence
    let reloaded_store = CookieStore::new(&cache_path);
    let reloaded = reloaded_store
        .get("wildberries")
        .await
        .expect("Session should be loaded from disk");
    assert_eq!(reloaded, session);

    // Case-insensitivity check
    let case_test = reloaded_store
        .get("WildBerries")
        .await
        .expect("Lookup should be case-insensitive");
    assert_eq!(case_test, session);

    // Test remove
    reloaded_store.remove("wildberries").await.unwrap();
    assert!(reloaded_store.get("wildberries").await.is_none());

    // File should reflect removal
    let third_store = CookieStore::new(&cache_path);
    assert!(third_store.get("wildberries").await.is_none());

    // Clean up
    let _ = std::fs::remove_dir_all(cache_path.parent().unwrap());
}

#[tokio::test]
async fn test_browser_headers_generation() {
    // Default headers without session
    let headers = build_browser_headers(None);
    assert_eq!(
        headers.get("user-agent").unwrap().to_str().unwrap(),
        DEFAULT_DESKTOP_UA
    );
    assert_eq!(
        headers.get("sec-ch-ua").unwrap().to_str().unwrap(),
        SEC_CH_UA
    );
    assert_eq!(
        headers.get("sec-ch-ua-platform").unwrap().to_str().unwrap(),
        "\"Linux\""
    );
    assert!(headers.get("cookie").is_none());

    // Headers with active session
    let session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "x_wbaas_token=abc; _wbauid=999".to_string(),
        expires_at: 9999999999,
        user_agent: "CustomUA/2.0".to_string(),
    };

    let session_headers = build_browser_headers(Some(&session));
    assert_eq!(
        session_headers.get("user-agent").unwrap().to_str().unwrap(),
        "CustomUA/2.0"
    );
    assert_eq!(
        session_headers.get("cookie").unwrap().to_str().unwrap(),
        "x_wbaas_token=abc; _wbauid=999"
    );
}

#[tokio::test]
async fn test_session_manager_cached_hit() {
    let cache_path = unique_temp_cache_path("mgr-cached");
    let store = CookieStore::new(&cache_path);
    let solver = ru_marketplace_mcp::session::NativeCdpSolver::default();
    let manager = SessionManager::new(store.clone(), solver);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let session = SessionData {
        marketplace: "ozon".to_string(),
        cookies: "__Secure-ETC=test".to_string(),
        expires_at: now + 3600,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };

    store.insert(session.clone()).await.unwrap();

    let fetched = manager.get_or_refresh("ozon").await.unwrap();
    assert_eq!(fetched, session);

    let _ = std::fs::remove_dir_all(cache_path.parent().unwrap());
}
