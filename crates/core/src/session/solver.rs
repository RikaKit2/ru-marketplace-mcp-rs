use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::cookies::SessionData;
use crate::domain::{Marketplace, ScraperError};

/// Universal session status information for monitoring and MCP tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusInfo {
    pub marketplace: Marketplace,
    pub solver_active: bool,
    pub has_cached_cookies: bool,
    pub cookies_expired: bool,
    pub expires_at: Option<u64>,
    pub details: String,
}

/// Abstract interface for marketplace session solvers.
#[async_trait]
pub trait SessionSolver: Send + Sync {
    /// Return the marketplace handled by this solver.
    fn marketplace(&self) -> Marketplace;

    /// Check if a persistent session or solver background process is currently active.
    async fn is_active(&self) -> bool;

    /// Explicitly start and warm up an active session / browser solver for fast execution.
    async fn start_session(&self) -> Result<String, ScraperError>;

    /// Explicitly terminate active session and free memory immediately.
    async fn stop_session(&self) -> Result<String, ScraperError>;

    /// Return comprehensive status information for this solver.
    async fn status(&self) -> SessionStatusInfo;

    /// Harvest fresh session credentials / cookies on-demand.
    async fn harvest(&self) -> Result<SessionData, ScraperError>;

    /// Invalidate existing session credentials.
    async fn invalidate(&self) -> Result<(), ScraperError>;
}
