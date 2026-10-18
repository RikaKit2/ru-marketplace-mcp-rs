use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{
    Marketplace, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
};
use ru_marketplace_core::session::SessionManager;
use ru_marketplace_core::transport::apply_browser_fingerprint;
use ru_marketplace_solver::NativeCdpSolver;

use super::parser::{parse_card_json, parse_composer_json};

pub const OZON_COMPOSER_URL: &str = "https://www.ozon.ru/api/entrypoint-api.bx/page/json/v2";

/// Ozon marketplace connector utilizing the Composer JSON API with Chromium solver integration.
#[derive(Debug, Clone)]
pub struct OzonConnector {
    client: reqwest::Client,
    session_manager: Arc<SessionManager>,
    solver: Arc<NativeCdpSolver>,
}

impl Default for OzonConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl OzonConnector {
    pub fn new() -> Self {
        let solver = Arc::new(NativeCdpSolver::default());
        let session_manager = Arc::new(SessionManager::default());
        Self::with_solver(session_manager, solver)
    }

    pub fn with_solver(session_manager: Arc<SessionManager>, solver: Arc<NativeCdpSolver>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .cookie_store(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            session_manager,
            solver,
        }
    }
}

#[async_trait]
impl MarketplaceConnector for OzonConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Ozon
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let mut url_param = format!("/search/?text={trimmed}&__rr=1");

        // Append price filters
        match (filters.min_price, filters.max_price) {
            (Some(min), Some(max)) => {
                url_param.push_str(&format!("&currency_price={min}.000%3B{max}.000"));
            }
            (Some(min), None) => {
                url_param.push_str(&format!("&currency_price={min}.000%3B"));
            }
            (None, Some(max)) => {
                url_param.push_str(&format!("&currency_price=%3B{max}.000"));
            }
            (None, None) => {}
        }

        // Append sorting
        if let Some(sort) = filters.sort {
            match sort {
                SortOrder::PriceAsc => url_param.push_str("&sorting=price"),
                SortOrder::PriceDesc => url_param.push_str("&sorting=price_desc"),
                SortOrder::Rating => url_param.push_str("&sorting=rating"),
                SortOrder::Discount => url_param.push_str("&sorting=discount"),
                SortOrder::Reviews | SortOrder::Popular => {}
            }
        }

        let full_url = format!("{OZON_COMPOSER_URL}?url={url_param}");
        let session = self.session_manager.get_session("ozon").await;

        let body = if let Some(ref sess) = session {
            // Attempt direct HTTP with valid cached cookies
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &url_param)]);
            match apply_browser_fingerprint(req, Some(sess)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let status = resp.status();
                    let text = resp.text().await?;
                    if text.contains("challenge-platform") || text.contains("cf-turnstile") {
                        info!(
                            "Ozon challenge detected with cached cookies, falling back to Chromium solver tab..."
                        );
                        self.session_manager
                            .invalidate_if_rejected("ozon", status, &text)
                            .await;
                        self.solver
                            .fetch_in_tab(Marketplace::Ozon, &full_url)
                            .await?
                    } else {
                        text
                    }
                }
                Ok(resp) => {
                    info!("Ozon direct request failed, falling back to Chromium solver tab...");
                    self.session_manager
                        .invalidate_if_rejected("ozon", resp.status(), "")
                        .await;
                    self.solver
                        .fetch_in_tab(Marketplace::Ozon, &full_url)
                        .await?
                }
                Err(_) => {
                    info!(
                        "Ozon direct request network error, falling back to Chromium solver tab..."
                    );
                    self.solver
                        .fetch_in_tab(Marketplace::Ozon, &full_url)
                        .await?
                }
            }
        } else {
            match self.solver.fetch_in_tab(Marketplace::Ozon, &full_url).await {
                Ok(json) if !json.contains("incidentId") && !json.contains("incident_id") => json,
                _ => {
                    let page_url = format!("https://www.ozon.ru/search/?text={trimmed}");
                    self.solver
                        .navigate_and_extract_html(Marketplace::Ozon, &page_url)
                        .await?
                }
            }
        };

        let mut offers = parse_composer_json(&body, limit)?;

        if filters.sort == Some(SortOrder::Reviews) {
            offers.sort_by_key(|b| std::cmp::Reverse(b.review_count.unwrap_or(0)));
        }

        if let Some(min) = filters.min_price {
            offers.retain(|o| o.price_tier.price() >= min);
        }
        if let Some(max) = filters.max_price {
            offers.retain(|o| o.price_tier.price() <= max);
        }

        Ok(offers)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let id = item_id.trim();
        let url_param = format!("/product/{id}/?__rr=1");
        let full_url = format!("{OZON_COMPOSER_URL}?url={url_param}");
        let session = self.session_manager.get_session("ozon").await;

        let body = if let Some(ref sess) = session {
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &url_param)]);
            match apply_browser_fingerprint(req, Some(sess)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let status = resp.status();
                    let text = resp.text().await?;
                    if text.contains("challenge-platform") || text.contains("cf-turnstile") {
                        info!(
                            "Ozon card challenge detected, falling back to Chromium solver tab..."
                        );
                        self.session_manager
                            .invalidate_if_rejected("ozon", status, &text)
                            .await;
                        self.solver
                            .fetch_in_tab(Marketplace::Ozon, &full_url)
                            .await?
                    } else {
                        text
                    }
                }
                Ok(resp) => {
                    info!("Ozon card request failed, falling back to Chromium solver tab...");
                    self.session_manager
                        .invalidate_if_rejected("ozon", resp.status(), "")
                        .await;
                    self.solver
                        .fetch_in_tab(Marketplace::Ozon, &full_url)
                        .await?
                }
                Err(_) => {
                    info!("Ozon card network error, falling back to Chromium solver tab...");
                    self.solver
                        .fetch_in_tab(Marketplace::Ozon, &full_url)
                        .await?
                }
            }
        } else {
            match self.solver.fetch_in_tab(Marketplace::Ozon, &full_url).await {
                Ok(json) if !json.contains("incidentId") && !json.contains("incident_id") => json,
                _ => {
                    let page_url = format!("https://www.ozon.ru/product/{id}/");
                    self.solver
                        .navigate_and_extract_html(Marketplace::Ozon, &page_url)
                        .await?
                }
            }
        };

        parse_card_json(&body, id)
    }
}
