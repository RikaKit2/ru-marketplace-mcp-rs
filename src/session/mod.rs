pub mod cookies;
pub mod native_solver;

use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::domain::ScraperError;
pub use cookies::{CookieStore, SessionData};
pub use native_solver::NativeCdpSolver;

/// Central coordinator for marketplace session credentials and lazy token refreshing.
#[derive(Debug, Clone)]
pub struct SessionManager {
    store: CookieStore,
    solver: NativeCdpSolver,
    refresh_lock: Arc<Mutex<()>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(CookieStore::default(), NativeCdpSolver::default())
    }
}

impl SessionManager {
    /// Create a SessionManager with specific store and solver backends.
    pub fn new(store: CookieStore, solver: NativeCdpSolver) -> Self {
        Self {
            store,
            solver,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Retrieve valid unexpired session data for a marketplace without triggering a refresh.
    pub async fn get_session(&self, marketplace: &str) -> Option<SessionData> {
        self.store.get(marketplace).await
    }

    /// Retrieve existing valid session, or lazily harvest fresh tokens via native CDP solver.
    pub async fn get_or_refresh(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        if let Some(session) = self.store.get(marketplace).await {
            return Ok(session);
        }

        let _guard = self.refresh_lock.lock().await;
        if let Some(session) = self.store.get(marketplace).await {
            return Ok(session);
        }

        self.harvest_and_store(marketplace).await
    }

    /// Forcefully harvest fresh session tokens and update the persistent cache.
    pub async fn refresh(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        let _guard = self.refresh_lock.lock().await;
        self.harvest_and_store(marketplace).await
    }

    async fn harvest_and_store(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        info!(
            marketplace = %marketplace,
            "Refreshing session tokens on demand..."
        );

        let session = if marketplace.eq_ignore_ascii_case("ozon") {
            self.solver.harvest_ozon().await?
        } else {
            return Err(ScraperError::NotFound {
                query: format!(
                    "Token refresh solver not implemented for marketplace '{marketplace}'"
                ),
            });
        };

        if let Err(e) = self.store.insert(session.clone()).await {
            tracing::warn!(
                error = %e,
                marketplace = %marketplace,
                "Failed to write refreshed session to cookie cache"
            );
        }

        Ok(session)
    }

    /// Invalidate existing session for a marketplace.
    pub async fn invalidate(&self, marketplace: &str) {
        if let Err(e) = self.store.remove(marketplace).await {
            tracing::warn!(
                error = %e,
                marketplace = %marketplace,
                "Failed to remove invalidated session from cookie cache"
            );
        }
    }

    /// Fetch raw JSON from Ozon Composer API through solver when TLS/WAF blocks direct HTTP.
    pub async fn fetch_composer(&self, url_param: &str) -> Result<String, ScraperError> {
        self.solver.fetch_composer(url_param).await
    }

    /// Explicitly start and warm up an active Ozon browser session for fast batch processing (~250ms).
    pub async fn start_ozon_session(&self) -> Result<String, ScraperError> {
        self.solver.start_session().await
    }

    /// Explicitly terminate active Ozon browser session and free memory immediately.
    pub async fn stop_ozon_session(&self) -> Result<String, ScraperError> {
        self.solver.stop_session().await
    }
}
