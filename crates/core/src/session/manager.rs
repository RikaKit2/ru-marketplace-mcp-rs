use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use super::cookies::{CookieStore, SessionData};
use super::solver::{SessionSolver, SessionStatusInfo};
use crate::domain::{Marketplace, ScraperError};

/// Central coordinator for marketplace session credentials, lazy token refreshing,
/// and pluggable session solvers.
#[derive(Clone)]
pub struct SessionManager {
    store: CookieStore,
    solvers: Arc<RwLock<HashMap<Marketplace, Arc<dyn SessionSolver>>>>,
    refresh_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("store", &self.store)
            .finish()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(CookieStore::default())
    }
}

impl SessionManager {
    /// Create a SessionManager with a specific CookieStore.
    pub fn new(store: CookieStore) -> Self {
        Self {
            store,
            solvers: Arc::new(RwLock::new(HashMap::new())),
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Register a session solver for a marketplace.
    pub async fn register_solver(&self, solver: Arc<dyn SessionSolver>) {
        let mut solvers = self.solvers.write().await;
        solvers.insert(solver.marketplace(), solver);
    }

    /// Register a session solver synchronously (e.g. during initialization).
    pub fn register_solver_sync(&self, solver: Arc<dyn SessionSolver>) {
        if let Ok(mut solvers) = self.solvers.try_write() {
            solvers.insert(solver.marketplace(), solver);
        }
    }

    /// Retrieve valid unexpired session data for a marketplace without triggering a refresh.
    pub async fn get_session(&self, marketplace: &str) -> Option<SessionData> {
        self.store.get(marketplace).await
    }

    /// Retrieve existing valid session, or lazily harvest fresh tokens via registered solver.
    pub async fn get_or_refresh(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        if let Some(session) = self.get_session(marketplace).await {
            return Ok(session);
        }
        self.refresh(marketplace).await
    }

    /// Forcefully harvest fresh session tokens and update the persistent cache.
    pub async fn refresh(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        let _guard = self.refresh_lock.lock().await;

        if let Some(session) = self.get_session(marketplace).await {
            return Ok(session);
        }

        self.harvest_and_store(marketplace).await
    }

    async fn harvest_and_store(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        let target_marketplace = marketplace.parse::<Marketplace>().ok();
        let solver = match target_marketplace {
            Some(mp) => {
                let solvers = self.solvers.read().await;
                solvers.get(&mp).cloned()
            }
            None => None,
        };

        if let Some(solver) = solver {
            info!(marketplace = %marketplace, "Triggering session solver to harvest tokens...");
            let session_data = solver.harvest().await?;
            if let Err(err) = self.store.insert(session_data.clone()).await {
                warn!("Failed to persist harvested session to disk: {err}");
            }
            Ok(session_data)
        } else {
            Err(ScraperError::Config(format!(
                "No session solver registered for marketplace '{marketplace}'"
            )))
        }
    }

    /// Invalidate existing session for a marketplace.
    pub async fn invalidate(&self, marketplace: &str) {
        let _ = self.store.remove(marketplace).await;
        if let Ok(mp) = marketplace.parse::<Marketplace>() {
            let solvers = self.solvers.read().await;
            if let Some(solver) = solvers.get(&mp) {
                let _ = solver.invalidate().await;
            }
        }
    }

    /// Check if a response indicates that session tokens were rejected or challenged,
    /// and if so, reactively invalidate the cached session to force a fresh harvest on next call.
    pub async fn invalidate_if_rejected(
        &self,
        marketplace: &str,
        status: reqwest::StatusCode,
        body: &str,
    ) {
        let is_rejected = status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status.as_u16() == 498
            || body.contains("challenge-platform")
            || body.contains("cf-turnstile")
            || body.contains("qrator_js")
            || body.contains("incidentId")
            || body.contains("incident_id");

        if is_rejected {
            info!(
                marketplace = %marketplace,
                status = %status,
                "Reactive session invalidation triggered due to rejected tokens/challenge"
            );
            self.invalidate(marketplace).await;
        }
    }

    /// Start session for target marketplace ("all", "ozon", "wb", etc.).
    pub async fn start_session(&self, target: &str) -> Result<String, ScraperError> {
        let target_lower = target.to_lowercase();
        if target_lower == "all" {
            let solvers = self.solvers.read().await;
            if solvers.is_empty() {
                return Ok("No session solvers registered.".to_string());
            }
            let mut results = Vec::new();
            for (mp, solver) in solvers.iter() {
                match solver.start_session().await {
                    Ok(msg) => results.push(format!("[{mp}]: {msg}")),
                    Err(err) => results.push(format!("[{mp}] Error: {err}")),
                }
            }
            return Ok(results.join("\n"));
        }

        let mp = target_lower.parse::<Marketplace>().map_err(|e| {
            ScraperError::InvalidFilter(format!("Unknown marketplace '{target}': {e}"))
        })?;

        let solver = {
            let solvers = self.solvers.read().await;
            solvers.get(&mp).cloned()
        };

        match solver {
            Some(solver) => solver.start_session().await,
            None => Ok(format!(
                "Marketplace {mp} is ready (no background solver required)."
            )),
        }
    }

    /// Stop session for target marketplace ("all", "ozon", "wb", etc.).
    pub async fn stop_session(&self, target: &str) -> Result<String, ScraperError> {
        let target_lower = target.to_lowercase();
        if target_lower == "all" {
            let solvers = self.solvers.read().await;
            if solvers.is_empty() {
                return Ok("No active sessions to stop.".to_string());
            }
            let mut results = Vec::new();
            for (mp, solver) in solvers.iter() {
                match solver.stop_session().await {
                    Ok(msg) => results.push(format!("[{mp}]: {msg}")),
                    Err(err) => results.push(format!("[{mp}] Error: {err}")),
                }
            }
            return Ok(results.join("\n"));
        }

        let mp = target_lower.parse::<Marketplace>().map_err(|e| {
            ScraperError::InvalidFilter(format!("Unknown marketplace '{target}': {e}"))
        })?;

        let solver = {
            let solvers = self.solvers.read().await;
            solvers.get(&mp).cloned()
        };

        match solver {
            Some(solver) => solver.stop_session().await,
            None => Ok(format!("No background session solver active for {mp}.")),
        }
    }

    /// Get session status for target marketplace ("all", "ozon", "wb", etc.).
    pub async fn session_status(&self, target: &str) -> Vec<SessionStatusInfo> {
        let target_lower = target.to_lowercase();
        let solvers = self.solvers.read().await;

        let market_list: Vec<Marketplace> = if target_lower == "all" {
            vec![
                Marketplace::Ozon,
                Marketplace::Wildberries,
                Marketplace::Dns,
                Marketplace::Citilink,
            ]
        } else if let Ok(mp) = target_lower.parse::<Marketplace>() {
            vec![mp]
        } else {
            return Vec::new();
        };

        let mut statuses = Vec::new();
        for mp in market_list {
            if let Some(solver) = solvers.get(&mp) {
                let mut info = solver.status().await;
                if let Some(sess) = self.store.get(&mp.to_string()).await {
                    info.has_cached_cookies = true;
                    info.cookies_expired = sess.is_expired();
                    info.expires_at = Some(sess.expires_at);
                }
                statuses.push(info);
            } else {
                let sess = self.store.get(&mp.to_string()).await;
                statuses.push(SessionStatusInfo {
                    marketplace: mp,
                    solver_active: false,
                    has_cached_cookies: sess.is_some(),
                    cookies_expired: sess.as_ref().map(|s| s.is_expired()).unwrap_or(false),
                    expires_at: sess.map(|s| s.expires_at),
                    details: "Direct API connector (no solver registered)".to_string(),
                });
            }
        }
        statuses
    }
}
