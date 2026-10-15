use async_trait::async_trait;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, ORIGIN, REFERER, USER_AGENT};
use serde_json::Value;

use super::MarketplaceConnector;
use crate::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus, WafProtection,
};

const WB_DEFAULT_DEST: &str = "-1257786"; // Moscow region ID for valid prices/stocks
const WB_SEARCH_URL: &str = "https://search.wb.ru/exactmatch/ru/common/v9/search";
const WB_CARD_URL: &str = "https://card.wb.ru/cards/v4/detail";

/// Wildberries marketplace catalog connector.
#[derive(Debug, Clone)]
pub struct WildberriesConnector {
    client: reqwest::Client,
    dest: String,
}

impl Default for WildberriesConnector {
    fn default() -> Self {
        Self::new(WB_DEFAULT_DEST)
    }
}

impl WildberriesConnector {
    /// Create a new connector with custom destination region ID.
    pub fn new(dest: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("https://www.wildberries.ru"),
        );
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://www.wildberries.ru/"),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            dest: dest.into(),
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

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .client
            .get(WB_SEARCH_URL)
            .query(&[
                ("appType", "1"),
                ("curr", "rub"),
                ("dest", &self.dest),
                ("locale", "ru"),
                ("query", trimmed),
                ("resultset", "catalog"),
                ("page", "1"),
                ("spp", "30"),
            ])
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(5),
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: "Wildberries returned HTTP 403 Forbidden (Wbaas Anti-Bot)".to_string(),
            });
        }

        let body = response.text().await?;

        // Check if response is an anti-bot challenge page
        if body.contains("captcha") || body.contains("wbaas") || body.contains("Access denied") {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: "Wildberries captcha or Wbaas challenge detected".to_string(),
            });
        }

        Self::parse_products_json(&body, limit)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let id = item_id.trim();
        let response = self
            .client
            .get(WB_CARD_URL)
            .query(&[
                ("appType", "1"),
                ("curr", "rub"),
                ("dest", &self.dest),
                ("spp", "30"),
                ("nm", id),
            ])
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(5),
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: "Wildberries returned HTTP 403 Forbidden".to_string(),
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
