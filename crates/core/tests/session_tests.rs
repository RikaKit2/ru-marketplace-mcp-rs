use std::time::{SystemTime, UNIX_EPOCH};

use ru_marketplace_core::session::{CookieStore, SessionData, SessionManager};
use ru_marketplace_core::transport::{DEFAULT_DESKTOP_UA, SEC_CH_UA, build_browser_headers};

fn unique_temp_cache_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("ru-mcp-core-test-{name}-{nanos}"))
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
        cookies: "test=expired".to_string(),
        expires_at: now - 100,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(expired_session.is_expired());

    let expiring_soon_session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "test=soon".to_string(),
        expires_at: now + 30, // within 60s window
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(expiring_soon_session.is_expired());

    let valid_session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "test=valid".to_string(),
        expires_at: now + 3600,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };
    assert!(!valid_session.is_expired());
}

#[tokio::test]
async fn test_cookie_store_file_persistence() {
    let cache_path = unique_temp_cache_path("store-persistence");
    let store = CookieStore::new(&cache_path);

    assert!(store.get("wildberries").await.is_none());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let session = SessionData {
        marketplace: "wildberries".to_string(),
        cookies: "token=wb_12345; session=abc".to_string(),
        expires_at: now + 7200,
        user_agent: DEFAULT_DESKTOP_UA.to_string(),
    };

    store.insert(session.clone()).await.unwrap();

    let fetched = store
        .get("wildberries")
        .await
        .expect("Session should exist");
    assert_eq!(fetched, session);

    let reloaded_store = CookieStore::new(&cache_path);
    let reloaded = reloaded_store
        .get("wildberries")
        .await
        .expect("Session should be loaded from disk");
    assert_eq!(reloaded, session);

    let case_test = reloaded_store
        .get("WildBerries")
        .await
        .expect("Lookup should be case-insensitive");
    assert_eq!(case_test, session);

    reloaded_store.remove("wildberries").await.unwrap();
    assert!(reloaded_store.get("wildberries").await.is_none());

    let third_store = CookieStore::new(&cache_path);
    assert!(third_store.get("wildberries").await.is_none());

    let _ = std::fs::remove_dir_all(cache_path.parent().unwrap());
}

#[tokio::test]
async fn test_browser_headers_generation() {
    let headers_no_session = build_browser_headers(None);
    assert_eq!(
        headers_no_session.get("user-agent").unwrap(),
        DEFAULT_DESKTOP_UA
    );
    assert_eq!(headers_no_session.get("sec-ch-ua").unwrap(), SEC_CH_UA);
    assert!(headers_no_session.get("cookie").is_none());

    let session = SessionData {
        marketplace: "ozon".to_string(),
        cookies: "__Secure-ETC=abc123xyz; __Secure-user_id=456".to_string(),
        expires_at: 9999999999,
        user_agent: "CustomUserAgent/1.0".to_string(),
    };

    let headers_with_session = build_browser_headers(Some(&session));
    assert_eq!(
        headers_with_session.get("user-agent").unwrap(),
        "CustomUserAgent/1.0"
    );
    assert_eq!(
        headers_with_session.get("cookie").unwrap(),
        "__Secure-ETC=abc123xyz; __Secure-user_id=456"
    );
}

#[tokio::test]
async fn test_session_manager_cached_hit() {
    let cache_path = unique_temp_cache_path("mgr-cached");
    let store = CookieStore::new(&cache_path);
    let manager = SessionManager::new(store.clone());

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
