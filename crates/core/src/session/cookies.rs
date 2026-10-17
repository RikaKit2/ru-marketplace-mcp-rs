use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Session credentials harvested for a specific marketplace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionData {
    /// Marketplace identifier (e.g. "wildberries", "ozon").
    pub marketplace: String,
    /// Formatted cookie header string (e.g. "x_wbaas_token=...; __wbaas_token=...").
    pub cookies: String,
    /// Expiration timestamp in seconds since UNIX epoch.
    pub expires_at: u64,
    /// User-Agent string paired with these session cookies.
    pub user_agent: String,
}

impl SessionData {
    /// Check whether the session credentials have expired or are about to expire.
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Consider expired 60 seconds early to avoid mid-flight expiration
        now + 60 >= self.expires_at
    }
}

/// File-backed persistent cookie and session cache.
#[derive(Debug, Clone)]
pub struct CookieStore {
    path: PathBuf,
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
}

impl Default for CookieStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

impl CookieStore {
    /// Create a new CookieStore using the specified file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let sessions = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<HashMap<String, SessionData>>(&content)
                {
                    Ok(map) => {
                        debug!("Loaded {} session(s) from {:?}", map.len(), path);
                        map
                    }
                    Err(e) => {
                        warn!("Failed to deserialize cookie cache from {:?}: {e}", path);
                        HashMap::new()
                    }
                },
                Err(e) => {
                    warn!("Failed to read cookie cache file {:?}: {e}", path);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Self {
            path,
            sessions: Arc::new(RwLock::new(sessions)),
        }
    }

    /// Resolve default cache path: `~/.cache/ru-marketplace-mcp/cookies.json`
    /// or from `$RU_MARKETPLACE_COOKIE_CACHE` / `$XDG_CACHE_HOME`.
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("RU_MARKETPLACE_COOKIE_CACHE") {
            return PathBuf::from(path);
        }
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            return PathBuf::from(xdg)
                .join("ru-marketplace-mcp")
                .join("cookies.json");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".cache")
            .join("ru-marketplace-mcp")
            .join("cookies.json")
    }

    /// Retrieve valid unexpired session data for a marketplace.
    pub async fn get(&self, marketplace: &str) -> Option<SessionData> {
        let key = marketplace.to_lowercase();
        let guard = self.sessions.read().await;
        if let Some(session) = guard.get(&key) {
            if !session.is_expired() {
                debug!("Valid cached session found for '{key}'");
                return Some(session.clone());
            }
            debug!("Cached session for '{key}' is expired");
        }
        None
    }

    /// Store a session and persist cache to disk.
    pub async fn insert(&self, session: SessionData) -> Result<(), std::io::Error> {
        let key = session.marketplace.to_lowercase();
        {
            let mut guard = self.sessions.write().await;
            guard.insert(key, session);
        }
        self.save().await
    }

    /// Invalidate/remove session for a marketplace.
    pub async fn remove(&self, marketplace: &str) -> Result<(), std::io::Error> {
        let key = marketplace.to_lowercase();
        {
            let mut guard = self.sessions.write().await;
            guard.remove(&key);
        }
        self.save().await
    }

    /// Persist in-memory session map to file.
    async fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let guard = self.sessions.read().await;
        let json = serde_json::to_string_pretty(&*guard)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        tokio::fs::write(&self.path, json).await?;
        debug!("Persisted {} session(s) to {:?}", guard.len(), self.path);
        Ok(())
    }
}
