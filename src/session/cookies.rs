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
        // 60-second safety margin
        self.expires_at <= now + 60
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
                Ok(content) => serde_json::from_str::<HashMap<String, SessionData>>(&content)
                    .unwrap_or_else(|err| {
                        warn!("Failed to parse cookie cache at {:?}: {err}", path);
                        HashMap::new()
                    }),
                Err(err) => {
                    warn!("Failed to read cookie cache at {:?}: {err}", path);
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
        if let Ok(env_path) = std::env::var("RU_MARKETPLACE_COOKIE_CACHE") {
            return PathBuf::from(env_path);
        }

        let base_dir = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|_| std::env::temp_dir());

        base_dir.join("ru-marketplace-mcp").join("cookies.json")
    }

    /// Retrieve valid unexpired session data for a marketplace.
    pub async fn get(&self, marketplace: &str) -> Option<SessionData> {
        let key = marketplace.to_lowercase();
        let lock = self.sessions.read().await;
        let session = lock.get(&key)?;

        if session.is_expired() {
            debug!(
                "Session for '{marketplace}' in cache has expired (expires_at={})",
                session.expires_at
            );
            None
        } else {
            Some(session.clone())
        }
    }

    /// Store a session and persist cache to disk.
    pub async fn insert(&self, session: SessionData) -> Result<(), std::io::Error> {
        let key = session.marketplace.to_lowercase();
        {
            let mut lock = self.sessions.write().await;
            lock.insert(key, session);
        }
        self.save().await
    }

    /// Invalidate/remove session for a marketplace.
    pub async fn remove(&self, marketplace: &str) -> Result<(), std::io::Error> {
        let key = marketplace.to_lowercase();
        {
            let mut lock = self.sessions.write().await;
            lock.remove(&key);
        }
        self.save().await
    }

    /// Persist in-memory session map to file.
    async fn save(&self) -> Result<(), std::io::Error> {
        let snapshot = {
            let lock = self.sessions.read().await;
            lock.clone()
        };

        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&self.path, json.as_bytes()).await?;
        debug!("Saved {} sessions to {:?}", snapshot.len(), self.path);
        Ok(())
    }
}
