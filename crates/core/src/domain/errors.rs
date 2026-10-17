use thiserror::Error;

use super::marketplaces::WafProtection;

/// Unified errors produced by marketplace scrapers and connectors.
#[derive(Debug, Error)]
pub enum ScraperError {
    #[error("WAF protection triggered: {waf}, details: {details}")]
    WafBlocked { waf: WafProtection, details: String },

    #[error("Rate limited, retry after: {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("Resource not found for query/id: '{query}'")]
    NotFound { query: String },

    #[error("HTTP network transport error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Parser drift or unparseable payload: {0}")]
    Parse(String),

    #[error("Request timed out")]
    Timeout,

    #[error("Configuration or session solver error: {0}")]
    Config(String),

    #[error("Invalid filter or query parameter: {0}")]
    InvalidFilter(String),
}

impl ScraperError {
    /// Return whether this error indicates anti-bot or WAF challenge.
    pub fn is_waf_blocked(&self) -> bool {
        matches!(self, Self::WafBlocked { .. })
    }

    /// Return whether this error is rate-limited (HTTP 429).
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimited { .. })
    }

    /// Human-friendly classification tag.
    pub fn status_tag(&self) -> &'static str {
        match self {
            Self::WafBlocked { .. } => "waf_blocked",
            Self::RateLimited { .. } => "rate_limited",
            Self::NotFound { .. } => "not_found",
            Self::Network(_) => "network_error",
            Self::Parse(_) => "parse_error",
            Self::Timeout => "timeout",
            Self::Config(_) => "config_error",
            Self::InvalidFilter(_) => "invalid_filter",
        }
    }
}
