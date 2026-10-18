use std::time::Duration;
use tracing::warn;

use crate::domain::ScraperError;

/// Execute an HTTP request with exponential backoff retry for transient network errors.
/// Retries on connection errors, timeouts, and 502/503/504 server errors.
/// Does NOT retry on 4xx client errors (400, 401, 403, 404).
pub async fn send_with_transient_retry<F, Fut>(
    max_retries: usize,
    initial_backoff: Duration,
    mut make_req: F,
) -> Result<reqwest::Response, ScraperError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut backoff = initial_backoff;
    let mut last_err = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }

        match make_req().await {
            Ok(resp) => {
                let status = resp.status();
                if (status == reqwest::StatusCode::BAD_GATEWAY
                    || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                    || status == reqwest::StatusCode::GATEWAY_TIMEOUT)
                    && attempt < max_retries
                {
                    warn!(
                        attempt = attempt + 1,
                        status = %status,
                        "Transient server error received; retrying..."
                    );
                    continue;
                }
                return Ok(resp);
            }
            Err(err) => {
                if (err.is_connect() || err.is_timeout()) && attempt < max_retries {
                    warn!(
                        attempt = attempt + 1,
                        error = %err,
                        "Transient network error; retrying..."
                    );
                    last_err = Some(err);
                    continue;
                }
                return Err(ScraperError::Network(err));
            }
        }
    }

    if let Some(err) = last_err {
        Err(ScraperError::Network(err))
    } else {
        Err(ScraperError::Timeout)
    }
}
