use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

use super::MarketplaceConnector;
use crate::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
    StockStatus, WafProtection,
};
use crate::session::SessionManager;
use crate::transport::apply_browser_fingerprint;

const WB_DEFAULT_DEST: &str = "-1257786"; // Moscow region ID for valid prices/stocks
const WB_SEARCH_URL: &str = "https://search.wb.ru/exactmatch/ru/common/v9/search";
const WB_CARD_URL: &str = "https://card.wb.ru/cards/v4/detail";

/// Wildberries marketplace catalog connector.
#[derive(Debug, Clone)]
pub struct WildberriesConnector {
    client: reqwest::Client,
    dest: String,
    session_manager: Arc<SessionManager>,
}

impl Default for WildberriesConnector {
    fn default() -> Self {
        Self::new(WB_DEFAULT_DEST)
    }
}

impl WildberriesConnector {
    /// Create a new connector with custom destination region ID and default session manager.
    pub fn new(dest: impl Into<String>) -> Self {
        Self::with_session_manager(dest, Arc::new(SessionManager::default()))
    }

    /// Create a new connector with custom destination and shared session manager.
    pub fn with_session_manager(
        dest: impl Into<String>,
        session_manager: Arc<SessionManager>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            dest: dest.into(),
            session_manager,
        }
    }

    /// Parse raw Wildberries v9 search or v4 card JSON payload into ProductOffers.
    pub fn parse_products_json(
        json_str: &str,
        limit: usize,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let root: Value = serde_json::from_str(json_str)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse WB JSON: {e}")))?;

        // Search v9 root has "data": { "products": [...] }, card v4 root has "data": { "products": [...] }
        let products = root
            .get("data")
            .and_then(|d| d.get("products"))
            .and_then(|p| p.as_array())
            .or_else(|| root.get("products").and_then(|p| p.as_array()));

        let Some(products_array) = products else {
            return Ok(Vec::new());
        };

        let mut offers = Vec::with_capacity(limit.min(products_array.len()));

        for p in products_array.iter().take(limit) {
            let Some(id) = p.get("id").and_then(|v| v.as_u64()) else {
                continue;
            };

            let title = p
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();

            if title.is_empty() {
                continue;
            }

            // Extract price from sizes[0].price.{product, basic, total} (in kopecks)
            let (price_kopecks, original_kopecks) = Self::extract_prices(p);

            let Some(kopecks) = price_kopecks else {
                continue; // Skip items without a live sellable price
            };

            let price_tier = PriceTier::from_kopecks(kopecks);
            let original_price = original_kopecks.map(|k| (k + 50) / 100);

            let rating = p
                .get("reviewRating")
                .or_else(|| p.get("rating"))
                .and_then(|v| v.as_f64())
                .map(|r| r as f32);

            let review_count = p
                .get("feedbacks")
                .or_else(|| p.get("reviewsCount"))
                .and_then(|v| v.as_u64())
                .map(|c| c as u32);

            let stock = match p.get("totalQuantity").and_then(|v| v.as_u64()) {
                Some(0) => StockStatus::OutOfStock,
                Some(qty) => StockStatus::InStock(qty as u32),
                None => StockStatus::Unknown,
            };

            let seller = p
                .get("supplier")
                .or_else(|| p.get("brand"))
                .and_then(|v| v.as_str())
                .map(str::to_string);

            offers.push(ProductOffer {
                marketplace: Marketplace::Wildberries,
                title,
                url: format!("https://www.wildberries.ru/catalog/{id}/detail.aspx"),
                price_tier,
                original_price,
                currency: Currency::Rub,
                rating,
                review_count,
                stock,
                seller,
                product_id: Some(id.to_string()),
            });
        }

        Ok(offers)
    }

    /// Extract (current_price_kopecks, original_price_kopecks) from product object.
    fn extract_prices(p: &Value) -> (Option<u64>, Option<u64>) {
        if let Some(sizes) = p.get("sizes").and_then(|v| v.as_array()) {
            for sz in sizes {
                if let Some(price_obj) = sz.get("price") {
                    let current = price_obj
                        .get("product")
                        .or_else(|| price_obj.get("total"))
                        .and_then(|v| v.as_u64())
                        .filter(|&v| v > 0);

                    let basic = price_obj
                        .get("basic")
                        .and_then(|v| v.as_u64())
                        .filter(|&v| v > 0);

                    if current.is_some() {
                        return (current, basic.or(current));
                    }
                }
            }
        }

        // Fallback to top-level priceU / salePriceU legacy fields
        let sale = p
            .get("salePriceU")
            .and_then(|v| v.as_u64())
            .filter(|&v| v > 0);
        let base = p.get("priceU").and_then(|v| v.as_u64()).filter(|&v| v > 0);

        if sale.is_some() || base.is_some() {
            let current = sale.or(base);
            let original = base.or(sale);
            return (current, original);
        }

        (None, None)
    }
}

#[async_trait]
impl MarketplaceConnector for WildberriesConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Wildberries
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

        let mut query_params: Vec<(&str, String)> = vec![
            ("appType", "1".to_string()),
            ("curr", "rub".to_string()),
            ("dest", self.dest.clone()),
            ("locale", "ru".to_string()),
            ("query", trimmed.to_string()),
            ("resultset", "catalog".to_string()),
            ("page", "1".to_string()),
            ("spp", "30".to_string()),
        ];

        // Append price filters (priceU in kopecks)
        match (filters.min_price, filters.max_price) {
            (Some(min), Some(max)) => {
                query_params.push(("priceU", format!("{};{}", min * 100, max * 100)));
            }
            (Some(min), None) => {
                query_params.push(("priceU", format!("{};", min * 100)));
            }
            (None, Some(max)) => {
                query_params.push(("priceU", format!(";{}", max * 100)));
            }
            (None, None) => {}
        }

        // Append sorting
        if let Some(sort) = filters.sort {
            match sort {
                SortOrder::PriceAsc => query_params.push(("sort", "priceup".to_string())),
                SortOrder::PriceDesc => query_params.push(("sort", "pricedown".to_string())),
                SortOrder::Rating => query_params.push(("sort", "rate".to_string())),
                SortOrder::Discount => query_params.push(("sort", "benefit".to_string())),
                SortOrder::Popular => query_params.push(("sort", "popular".to_string())),
                SortOrder::Reviews => {}
            }
        }

        let mut session = self.session_manager.get_session("wildberries").await;

        let send_search = |sess: Option<&crate::session::SessionData>| {
            let req = self.client.get(WB_SEARCH_URL).query(&query_params);
            apply_browser_fingerprint(req, sess)
        };

        let mut response = send_search(session.as_ref()).send().await?;
        let mut status = response.status();

        // Auto-refresh tokens on HTTP 403 (Forbidden) or HTTP 498 (Token challenge required)
        if status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498 {
            info!(
                status = %status,
                "Wildberries challenge/forbidden response; refreshing tokens and retrying..."
            );
            match self.session_manager.refresh("wildberries").await {
                Ok(new_session) => {
                    session = Some(new_session);
                    response = send_search(session.as_ref()).send().await?;
                    status = response.status();
                }
                Err(err) => {
                    warn!("Failed to refresh Wildberries session: {err}");
                }
            }
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(5),
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498 {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: format!("Wildberries returned HTTP {status} (Wbaas Anti-Bot)"),
            });
        }

        let body = response.text().await?;

        // Check if response is an anti-bot challenge page
        if body.contains("captcha") || body.contains("wbaas") || body.contains("Access denied") {
            // Attempt one refresh if challenge page returned with 200 OK
            if let Ok(new_session) = self.session_manager.refresh("wildberries").await {
                let retry_resp = send_search(Some(&new_session)).send().await?;
                let retry_body = retry_resp.text().await?;
                if !retry_body.contains("captcha") && !retry_body.contains("wbaas") {
                    return Self::parse_products_json(&retry_body, limit);
                }
            }
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: "Wildberries captcha or Wbaas challenge detected".to_string(),
            });
        }

        let mut offers = Self::parse_products_json(&body, limit)?;

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
        let mut session = self.session_manager.get_session("wildberries").await;

        let send_card = |sess: Option<&crate::session::SessionData>| {
            let req = self.client.get(WB_CARD_URL).query(&[
                ("appType", "1"),
                ("curr", "rub"),
                ("dest", &self.dest),
                ("spp", "30"),
                ("nm", id),
            ]);
            apply_browser_fingerprint(req, sess)
        };

        let mut response = send_card(session.as_ref()).send().await?;
        let mut status = response.status();

        if (status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498)
            && let Ok(new_session) = self.session_manager.refresh("wildberries").await
        {
            session = Some(new_session);
            response = send_card(session.as_ref()).send().await?;
            status = response.status();
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(5),
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498 {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: format!("Wildberries returned HTTP {status}"),
            });
        }

        let body = response.text().await?;
        let offers = Self::parse_products_json(&body, 1)?;

        offers
            .into_iter()
            .next()
            .ok_or_else(|| ScraperError::NotFound {
                query: id.to_string(),
            })
    }
}
