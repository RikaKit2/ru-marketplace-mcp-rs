use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

use super::MarketplaceConnector;
use crate::domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
    WafProtection,
};
use crate::session::SessionManager;
use crate::transport::apply_browser_fingerprint;

const OZON_COMPOSER_URL: &str = "https://www.ozon.ru/api/composer-api.bx/page/json/v2";

/// Ozon marketplace connector utilizing the Composer JSON API.
#[derive(Debug, Clone)]
pub struct OzonConnector {
    client: reqwest::Client,
    session_manager: Arc<SessionManager>,
}

impl Default for OzonConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl OzonConnector {
    pub fn new() -> Self {
        Self::with_session_manager(Arc::new(SessionManager::default()))
    }

    pub fn with_session_manager(session_manager: Arc<SessionManager>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            session_manager,
        }
    }

    /// Parse price strings like "1 499 ₽", "12 490 ₽", "1499" into integer rubles.
    pub fn parse_price_str(text: &str) -> Option<u64> {
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok().filter(|&p| p > 0)
    }

    /// Parse an Ozon Composer API JSON response.
    pub fn parse_composer_json(
        json_str: &str,
        limit: usize,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let root: Value = serde_json::from_str(json_str)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon JSON: {e}")))?;

        let widgets = match root.get("widgetStates") {
            Some(Value::Object(map)) => map,
            _ => return Ok(Vec::new()),
        };

        let mut offers = Vec::new();

        for (key, val) in widgets {
            if !key.starts_with("tileGridDesktop-") && !key.starts_with("searchResultsV2-") {
                continue;
            }

            // The widget state can be a raw JSON string or already parsed JSON object
            let widget_obj: Value = if let Some(s) = val.as_str() {
                serde_json::from_str(s).unwrap_or(Value::Null)
            } else {
                val.clone()
            };

            let Some(items) = widget_obj.get("items").and_then(|v| v.as_array()) else {
                continue;
            };

            for item in items {
                if offers.len() >= limit {
                    break;
                }

                if let Some(offer) = Self::parse_search_tile(item) {
                    offers.push(offer);
                }
            }

            if offers.len() >= limit {
                break;
            }
        }

        Ok(offers)
    }

    /// Parse a single search tile item from Ozon mainState atoms.
    fn parse_search_tile(item: &Value) -> Option<ProductOffer> {
        let sku = item
            .get("sku")
            .or_else(|| item.get("id"))
            .map(|v| match v {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty());

        let raw_link = item
            .get("action")
            .and_then(|a| a.get("link"))
            .and_then(|l| l.as_str())
            .unwrap_or_default();

        let url = if raw_link.starts_with("http") {
            raw_link.to_string()
        } else if !raw_link.is_empty() {
            format!(
                "https://www.ozon.ru{}",
                raw_link.split('?').next().unwrap_or(raw_link)
            )
        } else if let Some(id) = &sku {
            format!("https://www.ozon.ru/product/{id}")
        } else {
            return None;
        };

        let mut title = None;
        let mut price_current = None;
        let mut price_original = None;
        let mut has_card_price = false;
        let mut rating = None;
        let mut review_count = None;
        let mut stock = StockStatus::Unknown;

        if let Some(atoms) = item.get("mainState").and_then(|v| v.as_array()) {
            for atom in atoms {
                let atom_type = atom
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                match atom_type {
                    "textDS" => {
                        let text = atom
                            .get("textDS")
                            .and_then(|t| t.get("text"))
                            .and_then(|v| v.as_str());
                        let atom_id = atom.get("id").and_then(|v| v.as_str()).unwrap_or_default();

                        if title.is_none()
                            && (atom_id == "name" || text.is_some())
                            && let Some(t) = text
                        {
                            title = Some(t.trim().to_string());
                        }
                    }
                    "textAtom" => {
                        if title.is_none()
                            && let Some(t) = atom
                                .get("textAtom")
                                .and_then(|a| a.get("text"))
                                .and_then(|v| v.as_str())
                        {
                            title = Some(t.trim().to_string());
                        }
                    }
                    "priceV2" => {
                        if let Some(pv) = atom.get("priceV2") {
                            if let Some(prices) = pv.get("price").and_then(|v| v.as_array()) {
                                if let Some(first) = prices
                                    .first()
                                    .and_then(|p| p.get("text"))
                                    .and_then(|v| v.as_str())
                                {
                                    price_current = Self::parse_price_str(first);
                                }
                                if prices.len() >= 2
                                    && let Some(second) = prices
                                        .get(1)
                                        .and_then(|p| p.get("text"))
                                        .and_then(|v| v.as_str())
                                {
                                    price_original = Self::parse_price_str(second);
                                }
                            }
                            if let Some(card_tag) = pv.get("ozonCardTag").and_then(|v| v.as_bool())
                            {
                                has_card_price = card_tag;
                            }
                        }
                    }
                    "labelList" => {
                        if let Some(items) = atom
                            .get("labelList")
                            .and_then(|l| l.get("items"))
                            .and_then(|v| v.as_array())
                        {
                            for it in items {
                                if let Some(txt) = it.get("title").and_then(|v| v.as_str()) {
                                    if txt.contains("В наличии") {
                                        stock = StockStatus::InStock(1);
                                    } else if txt.contains("Нет в наличии") {
                                        stock = StockStatus::OutOfStock;
                                    } else if txt.contains('★') || txt.contains("отзыв") {
                                        let parts: Vec<&str> = txt.split('•').collect();
                                        if let Some(r_str) = parts.first()
                                            && let Ok(r) =
                                                r_str.replace('★', "").trim().parse::<f32>()
                                        {
                                            rating = Some(r);
                                        }
                                        if parts.len() > 1 {
                                            let digits: String = parts[1]
                                                .chars()
                                                .filter(|c| c.is_ascii_digit())
                                                .collect();
                                            if let Ok(cnt) = digits.parse::<u32>() {
                                                review_count = Some(cnt);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let title = title?;
        let price = price_current?;

        let price_tier = if has_card_price {
            PriceTier::MarketplaceCard {
                price,
                card: CardType::OzonCard,
            }
        } else {
            PriceTier::Base(price)
        };

        if stock == StockStatus::Unknown {
            stock = StockStatus::InStock(1);
        }

        Some(ProductOffer {
            marketplace: Marketplace::Ozon,
            title,
            url,
            price_tier,
            original_price: price_original,
            currency: Currency::Rub,
            rating,
            review_count,
            stock,
            seller: None,
            product_id: sku,
        })
    }
}

#[async_trait]
impl MarketplaceConnector for OzonConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Ozon
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let encoded_url = format!("/search/?text={}", urlencoding(trimmed));
        let mut session = self.session_manager.get_session("ozon").await;

        let send_search = |sess: Option<&crate::session::SessionData>| {
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &encoded_url)]);
            apply_browser_fingerprint(req, sess)
        };

        let mut response = send_search(session.as_ref()).send().await?;
        let mut status = response.status();

        if status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498 {
            info!(
                status = %status,
                "Ozon challenge/forbidden response; refreshing tokens and retrying..."
            );
            match self.session_manager.refresh("ozon").await {
                Ok(new_session) => {
                    session = Some(new_session);
                    response = send_search(session.as_ref()).send().await?;
                    status = response.status();
                }
                Err(err) => {
                    warn!("Failed to refresh Ozon session: {err}");
                }
            }
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(10),
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::CloudflareTurnstile,
                details: "Ozon returned HTTP 403 Forbidden (Cloudflare Challenge)".to_string(),
            });
        }

        let body = response.text().await?;

        // Check if response is Cloudflare challenge
        if body.contains("challenge-platform")
            || body.contains("__cf_bm")
            || body.contains("cf-turnstile")
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::CloudflareTurnstile,
                details: "Ozon Cloudflare Turnstile challenge detected".to_string(),
            });
        }

        Self::parse_composer_json(&body, limit)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let id = item_id.trim();
        let encoded_url = format!("/product/{id}/");
        let mut session = self.session_manager.get_session("ozon").await;

        let send_card = |sess: Option<&crate::session::SessionData>| {
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &encoded_url)]);
            apply_browser_fingerprint(req, sess)
        };

        let mut response = send_card(session.as_ref()).send().await?;
        let mut status = response.status();

        if (status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 498)
            && let Ok(new_session) = self.session_manager.refresh("ozon").await
        {
            session = Some(new_session);
            response = send_card(session.as_ref()).send().await?;
            status = response.status();
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::CloudflareTurnstile,
                details: "Ozon HTTP 403 on card view".to_string(),
            });
        }

        let body = response.text().await?;
        let offers = Self::parse_composer_json(&body, 1)?;

        offers
            .into_iter()
            .next()
            .ok_or_else(|| ScraperError::NotFound {
                query: id.to_string(),
            })
    }
}

fn urlencoding(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{:02X}", b));
        }
    }
    encoded
}
