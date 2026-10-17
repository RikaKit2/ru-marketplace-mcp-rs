use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

use super::MarketplaceConnector;
use crate::domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, ScraperError, SearchFilterOptions,
    SortOrder, StockStatus, WafProtection,
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
            .timeout(std::time::Duration::from_secs(25))
            .cookie_store(true)
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

        if let Some(inc) = root
            .get("incidentId")
            .or_else(|| root.get("incident_id"))
            .and_then(|v| v.as_str())
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Ozon security incident: {inc}"),
            });
        }

        let widgets = match root.get("widgetStates") {
            Some(Value::Object(map)) => map,
            _ => {
                if let Some(err) = root
                    .get("error")
                    .or_else(|| root.get("message"))
                    .and_then(|v| v.as_str())
                {
                    return Err(ScraperError::WafBlocked {
                        waf: WafProtection::Variti,
                        details: format!("Ozon error response: {err}"),
                    });
                }
                return Ok(Vec::new());
            }
        };

        let mut offers = Vec::new();

        for (key, val) in widgets {
            let is_search_widget = key.starts_with("tileGridDesktop-")
                || key.starts_with("searchResultsV2-")
                || key.starts_with("tileGrid-")
                || key.starts_with("catalog-")
                || key.starts_with("megaPaginator-");
            if !is_search_widget {
                continue;
            }
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
                        if let Some(t) = atom
                            .get("textAtom")
                            .and_then(|a| a.get("text"))
                            .and_then(|v| v.as_str())
                        {
                            let trimmed = t.trim();
                            if !trimmed.contains("осталось")
                                && !trimmed.contains("шт")
                                && !trimmed.contains("ед")
                                && !trimmed.ends_with('%')
                                && (title.is_none()
                                    || title.as_ref().is_some_and(|cur| cur.len() < trimmed.len()))
                            {
                                title = Some(trimmed.to_string());
                            }
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

    /// Parse an Ozon product card Composer API JSON response.
    pub fn parse_card_json(json_str: &str, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let root: Value = serde_json::from_str(json_str)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon card JSON: {e}")))?;

        if let Some(inc) = root
            .get("incidentId")
            .or_else(|| root.get("incident_id"))
            .and_then(|v| v.as_str())
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Variti,
                details: format!("Ozon security incident: {inc}"),
            });
        }

        let widgets = match root.get("widgetStates") {
            Some(Value::Object(map)) => map,
            _ => {
                if let Some(err) = root
                    .get("error")
                    .or_else(|| root.get("message"))
                    .and_then(|v| v.as_str())
                {
                    return Err(ScraperError::WafBlocked {
                        waf: WafProtection::Variti,
                        details: format!("Ozon error response: {err}"),
                    });
                }
                return Err(ScraperError::NotFound {
                    query: item_id.to_string(),
                });
            }
        };

        let mut title = String::new();
        let mut base_price = 0u64;
        let mut card_price = 0u64;
        let mut original_price = None;
        let mut is_available = true;
        let mut rating = None;
        let mut review_count = None;
        let mut seller = None;

        for (key, val) in widgets {
            let widget_obj: Value = if let Some(s) = val.as_str() {
                serde_json::from_str(s).unwrap_or(Value::Null)
            } else {
                val.clone()
            };

            if key.starts_with("webProductHeading") {
                if let Some(t) = widget_obj.get("title").and_then(|v| v.as_str()) {
                    title = t.to_string();
                }
            } else if key.starts_with("webPrice-") {
                if let Some(avail) = widget_obj.get("isAvailable").and_then(|v| v.as_bool()) {
                    is_available = avail;
                }
                if let Some(p_str) = widget_obj.get("price").and_then(|v| v.as_str()) {
                    base_price = Self::parse_price_str(p_str).unwrap_or(0);
                }
                if let Some(cp_str) = widget_obj.get("cardPrice").and_then(|v| v.as_str()) {
                    card_price = Self::parse_price_str(cp_str).unwrap_or(0);
                }
                if let Some(orig_str) = widget_obj.get("originalPrice").and_then(|v| v.as_str()) {
                    original_price = Self::parse_price_str(orig_str);
                }
            } else if key.starts_with("webReviewProductScore") {
                if let Some(score) = widget_obj.get("totalScore").and_then(|v| v.as_f64()) {
                    rating = Some(score as f32);
                }
                if let Some(count) = widget_obj.get("reviewsCount").and_then(|v| v.as_u64()) {
                    review_count = Some(count as u32);
                }
            } else if key.starts_with("webBrand")
                && seller.is_none()
                && let Some(brand_name) = widget_obj
                    .get("content")
                    .and_then(|c| c.get("title"))
                    .and_then(|t| t.get("text"))
                    .and_then(|arr| arr.get(0))
                    .and_then(|item| item.get("content"))
                    .and_then(|v| v.as_str())
            {
                seller = Some(brand_name.to_string());
            } else if key.starts_with("webCurrentSeller")
                && let Some(s_name) = widget_obj
                    .get("name")
                    .or_else(|| widget_obj.get("title"))
                    .and_then(|v| v.as_str())
            {
                seller = Some(s_name.to_string());
            }
        }

        if title.is_empty() {
            return Err(ScraperError::NotFound {
                query: item_id.to_string(),
            });
        }

        let price_tier = if card_price > 0 && card_price < base_price {
            PriceTier::MarketplaceCard {
                price: card_price,
                card: CardType::OzonCard,
            }
        } else if base_price > 0 {
            PriceTier::Base(base_price)
        } else if card_price > 0 {
            PriceTier::Base(card_price)
        } else {
            PriceTier::Base(0)
        };

        let stock = if is_available {
            StockStatus::InStock(1)
        } else {
            StockStatus::OutOfStock
        };

        Ok(ProductOffer {
            marketplace: Marketplace::Ozon,
            title,
            url: format!("https://www.ozon.ru/product/{item_id}/"),
            price_tier,
            original_price,
            currency: Currency::Rub,
            rating,
            review_count,
            stock,
            seller,
            product_id: Some(item_id.to_string()),
        })
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

        let mut url_param = format!("/search/?text={trimmed}");

        // Append price filters if specified
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

        // Append sorting if specified
        if let Some(sort) = filters.sort {
            match sort {
                SortOrder::PriceAsc => url_param.push_str("&sorting=price"),
                SortOrder::PriceDesc => url_param.push_str("&sorting=price_desc"),
                SortOrder::Rating => url_param.push_str("&sorting=rating"),
                SortOrder::Discount => url_param.push_str("&sorting=discount"),
                SortOrder::Reviews | SortOrder::Popular => {}
            }
        }

        let mut session = self.session_manager.get_session("ozon").await;

        if session.is_none()
            && let Ok(new_session) = self.session_manager.refresh("ozon").await
        {
            session = Some(new_session);
        }

        let send_search = |sess: Option<&crate::session::SessionData>| {
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &url_param)]);
            apply_browser_fingerprint(req, sess)
        };

        let body = match send_search(session.as_ref()).send().await {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await?;
                if text.contains("challenge-platform") || text.contains("cf-turnstile") {
                    info!(
                        "Ozon challenge page detected in direct HTTP body, falling back to Chromium solver..."
                    );
                    self.session_manager.fetch_composer(&url_param).await?
                } else {
                    text
                }
            }
            Ok(resp) => {
                info!(status = %resp.status(), "Ozon direct HTTP returned non-success, falling back to Chromium solver...");
                self.session_manager.fetch_composer(&url_param).await?
            }
            Err(err) => {
                info!(error = %err, "Ozon direct HTTP failed, falling back to Chromium solver...");
                self.session_manager.fetch_composer(&url_param).await?
            }
        };

        let mut offers = Self::parse_composer_json(&body, limit)?;

        // In-memory sort by review count if requested
        if filters.sort == Some(SortOrder::Reviews) {
            offers.sort_by_key(|b| std::cmp::Reverse(b.review_count.unwrap_or(0)));
        }

        // Downstream price boundary safety check
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
        let url_param = format!("/product/{id}/");
        let mut session = self.session_manager.get_session("ozon").await;

        if session.is_none()
            && let Ok(new_session) = self.session_manager.refresh("ozon").await
        {
            session = Some(new_session);
        }

        let send_card = |sess: Option<&crate::session::SessionData>| {
            let req = self
                .client
                .get(OZON_COMPOSER_URL)
                .query(&[("url", &url_param)]);
            apply_browser_fingerprint(req, sess)
        };

        let body = match send_card(session.as_ref()).send().await {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await?;
                if text.contains("challenge-platform") || text.contains("cf-turnstile") {
                    info!(
                        "Ozon challenge page detected on card view, falling back to Chromium solver..."
                    );
                    self.session_manager.fetch_composer(&url_param).await?
                } else {
                    text
                }
            }
            Ok(resp) => {
                info!(status = %resp.status(), "Ozon card direct HTTP returned non-success, falling back to Chromium solver...");
                self.session_manager.fetch_composer(&url_param).await?
            }
            Err(err) => {
                info!(error = %err, "Ozon card direct HTTP failed, falling back to Chromium solver...");
                self.session_manager.fetch_composer(&url_param).await?
            }
        };

        Self::parse_card_json(&body, id)
    }
}
