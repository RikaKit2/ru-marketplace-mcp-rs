use async_trait::async_trait;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

use super::MarketplaceConnector;
use crate::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus, WafProtection,
};

const CITILINK_SEARCH_URL: &str = "https://www.citilink.ru/search/";

/// Citilink marketplace catalog connector.
#[derive(Debug, Clone)]
pub struct CitilinkConnector {
    client: reqwest::Client,
}

impl Default for CitilinkConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl CitilinkConnector {
    pub fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            ),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Extract numeric digits from price strings like "15 990 ₽" -> 15990.
    pub fn parse_price_str(text: &str) -> Option<u64> {
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok().filter(|&p| p > 0)
    }

    /// Parse Citilink HTML or JSON catalog content.
    pub fn parse_catalog_data(
        content: &str,
        limit: usize,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        if let Ok(val) = serde_json::from_str::<Value>(content) {
            return Self::parse_json_products(&val, limit);
        }

        let mut offers = Vec::new();

        // 1. Check embedded JSON or JSON-LD
        for block in content.split("<script") {
            if let Some(json_start) = block.find("application/ld+json\">") {
                let rest = &block[json_start + "application/ld+json\">".len()..];
                if let Some(json_end) = rest.find("</script>") {
                    let json_blob = &rest[..json_end].trim();
                    if let Ok(parsed) = serde_json::from_str::<Value>(json_blob)
                        && let Ok(sub_offers) = Self::parse_json_products(&parsed, limit)
                    {
                        offers.extend(sub_offers);
                        if offers.len() >= limit {
                            offers.truncate(limit);
                            return Ok(offers);
                        }
                    }
                }
            }
        }

        // 2. Parse product card HTML tiles
        for part in content.split("product_data__gtm-js") {
            if offers.len() >= limit {
                break;
            }

            let title = extract_between(part, "title=\"", "\"")
                .or_else(|| extract_between(part, "data-params-name=\"", "\""))
                .map(|s| clean_html(&s));

            let link = extract_between(part, "href=\"/product/", "\"")
                .map(|id| format!("https://www.citilink.ru/product/{id}"))
                .or_else(|| {
                    extract_between(part, "href=\"", "\"")
                        .filter(|l| l.contains("/product/"))
                        .map(|l| format!("https://www.citilink.ru{l}"))
                });

            let price = extract_between(part, "data-params-price=\"", "\"")
                .and_then(|s| Self::parse_price_str(&s))
                .or_else(|| {
                    extract_between(part, "product-min-price", "₽")
                        .and_then(|s| Self::parse_price_str(&s))
                });

            if let (Some(t), Some(url), Some(p)) = (title, link, price)
                && !t.is_empty()
            {
                offers.push(ProductOffer {
                    marketplace: Marketplace::Citilink,
                    title: t,
                    url,
                    price_tier: PriceTier::Base(p),
                    original_price: None,
                    currency: Currency::Rub,
                    rating: None,
                    review_count: None,
                    stock: StockStatus::InStock(1),
                    seller: Some("Citilink".to_string()),
                    product_id: None,
                });
            }
        }

        Ok(offers)
    }

    fn parse_json_products(val: &Value, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let mut offers = Vec::new();

        let items = if let Some(arr) = val.as_array() {
            arr.clone()
        } else if let Some(items) = val.get("itemListElement").and_then(|v| v.as_array()) {
            items.clone()
        } else if let Some(products) = val.get("products").and_then(|p| p.as_array()) {
            products.clone()
        } else {
            vec![val.clone()]
        };

        for item in items.iter().take(limit) {
            let p = item.get("item").unwrap_or(item);

            let title = p
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim();
            if title.is_empty() {
                continue;
            }

            let url = p
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let price = p
                .get("offers")
                .and_then(|o| o.get("price"))
                .and_then(|v| match v {
                    Value::Number(n) => n.as_u64(),
                    Value::String(s) => Self::parse_price_str(s),
                    _ => None,
                })
                .or_else(|| p.get("price").and_then(|v| v.as_u64()));

            let Some(price_val) = price else {
                continue;
            };

            offers.push(ProductOffer {
                marketplace: Marketplace::Citilink,
                title: title.to_string(),
                url: if url.is_empty() {
                    "https://www.citilink.ru".to_string()
                } else {
                    url
                },
                price_tier: PriceTier::Base(price_val),
                original_price: None,
                currency: Currency::Rub,
                rating: None,
                review_count: None,
                stock: StockStatus::InStock(1),
                seller: Some("Citilink".to_string()),
                product_id: p
                    .get("sku")
                    .or_else(|| p.get("id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }

        Ok(offers)
    }
}

#[async_trait]
impl MarketplaceConnector for CitilinkConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Citilink
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .client
            .get(CITILINK_SEARCH_URL)
            .query(&[("text", trimmed)])
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(15),
            });
        }

        // Citilink returns 401 Unauthorized when Qrator challenge is active
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!("Citilink HTTP {status} (Qrator proof-of-work challenge active)"),
            });
        }

        let body = response.text().await?;

        if body.contains("qrator") || body.contains("qrator_js") || body.contains("data-qrator") {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "Citilink Qrator anti-bot challenge script detected".to_string(),
            });
        }

        Self::parse_catalog_data(&body, limit)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let id = item_id.trim();
        let url = format!("https://www.citilink.ru/product/{id}/");
        let response = self.client.get(&url).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!("Citilink HTTP {status} (Qrator challenge active on card)"),
            });
        }

        let body = response.text().await?;
        let offers = Self::parse_catalog_data(&body, 1)?;

        offers
            .into_iter()
            .next()
            .ok_or_else(|| ScraperError::NotFound {
                query: id.to_string(),
            })
    }
}

fn extract_between(haystack: &str, start_tag: &str, end_tag: &str) -> Option<String> {
    let start_idx = haystack.find(start_tag)? + start_tag.len();
    let rest = &haystack[start_idx..];
    if end_tag.is_empty() {
        return Some(rest.to_string());
    }
    let end_idx = rest.find(end_tag)?;
    Some(rest[..end_idx].to_string())
}

fn clean_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
