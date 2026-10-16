use async_trait::async_trait;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

use super::MarketplaceConnector;
use crate::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
    StockStatus, WafProtection,
};

const DNS_SEARCH_URL: &str = "https://www.dns-shop.ru/search/";

/// DNS-Shop marketplace connector.
#[derive(Debug, Clone)]
pub struct DnsConnector {
    client: reqwest::Client,
}

impl Default for DnsConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsConnector {
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

    /// Extract numeric digits from price strings like "49 999 ₽" -> 49999.
    pub fn parse_price_str(text: &str) -> Option<u64> {
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok().filter(|&p| p > 0)
    }

    /// Parse DNS catalog JSON or HTML microdata into ProductOffers.
    pub fn parse_catalog_data(
        content: &str,
        limit: usize,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        // If content is JSON (e.g. from internal API or cached state)
        if let Ok(val) = serde_json::from_str::<Value>(content) {
            return Self::parse_json_products(&val, limit);
        }

        // HTML parser: scan for product items in DOM layout or JSON-LD embedded data
        let mut offers = Vec::new();

        // 1. Look for embedded JSON-LD or initial data script tags
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

        // 2. Scan catalog-product HTML blocks
        for part in content.split("catalog-product ") {
            if offers.len() >= limit {
                break;
            }

            // Look for data-product="..." or title / price tags
            let title = extract_between(part, "catalog-product__name", "</a>")
                .and_then(|s| extract_between(&s, ">", ""))
                .or_else(|| extract_between(part, "data-product-title=\"", "\""))
                .map(|s| clean_html(&s));

            let link = extract_between(part, "href=\"/product/", "\"")
                .map(|id| format!("https://www.dns-shop.ru/product/{id}"))
                .or_else(|| {
                    extract_between(part, "href=\"", "\"")
                        .filter(|l| l.contains("/product/"))
                        .map(|l| format!("https://www.dns-shop.ru{l}"))
                });

            let price = extract_between(part, "product-buy__price", "₽")
                .or_else(|| extract_between(part, "product-min-price", "₽"))
                .and_then(|s| Self::parse_price_str(&s));

            if let (Some(t), Some(url), Some(p)) = (title, link, price)
                && !t.is_empty()
            {
                offers.push(ProductOffer {
                    marketplace: Marketplace::Dns,
                    title: t,
                    url,
                    price_tier: PriceTier::Base(p),
                    original_price: None,
                    currency: Currency::Rub,
                    rating: None,
                    review_count: None,
                    stock: StockStatus::InStock(1),
                    seller: Some("DNS".to_string()),
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
        } else if let Some(products) = val
            .get("data")
            .and_then(|d| d.get("products"))
            .and_then(|p| p.as_array())
        {
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
                marketplace: Marketplace::Dns,
                title: title.to_string(),
                url: if url.is_empty() {
                    "https://www.dns-shop.ru".to_string()
                } else {
                    url
                },
                price_tier: PriceTier::Base(price_val),
                original_price: None,
                currency: Currency::Rub,
                rating: None,
                review_count: None,
                stock: StockStatus::InStock(1),
                seller: Some("DNS".to_string()),
                product_id: p.get("sku").and_then(|v| v.as_str()).map(str::to_string),
            });
        }

        Ok(offers)
    }
}

#[async_trait]
impl MarketplaceConnector for DnsConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Dns
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

        let response = self
            .client
            .get(DNS_SEARCH_URL)
            .query(&[("q", trimmed)])
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ScraperError::RateLimited {
                retry_after_secs: Some(15),
            });
        }

        // DNS returns 401 Unauthorized when Qrator challenge is active
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!("DNS-Shop HTTP {status} (Qrator proof-of-work challenge active)"),
            });
        }

        let body = response.text().await?;

        if body.contains("qrator") || body.contains("qrator_js") || body.contains("data-qrator") {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "DNS-Shop Qrator anti-bot challenge script detected".to_string(),
            });
        }

        let mut offers = Self::parse_catalog_data(&body, limit)?;

        if let Some(sort) = filters.sort {
            match sort {
                SortOrder::PriceAsc => offers.sort_by_key(|o| o.price_tier.price()),
                SortOrder::PriceDesc => {
                    offers.sort_by_key(|b| std::cmp::Reverse(b.price_tier.price()));
                }
                SortOrder::Rating => offers.sort_by(|a, b| {
                    b.rating
                        .unwrap_or(0.0)
                        .partial_cmp(&a.rating.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
                SortOrder::Reviews => {
                    offers.sort_by_key(|b| std::cmp::Reverse(b.review_count.unwrap_or(0)));
                }
                SortOrder::Discount | SortOrder::Popular => {}
            }
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
        let url = format!("https://www.dns-shop.ru/product/{id}/");
        let response = self.client.get(&url).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!("DNS-Shop HTTP {status} (Qrator challenge active on card)"),
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
