use async_trait::async_trait;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
    StockStatus, WafProtection,
};
use ru_marketplace_solver::NativeCdpSolver;

pub const CITILINK_SEARCH_URL: &str = "https://www.citilink.ru/search/";

/// Citilink marketplace catalog connector.
#[derive(Debug, Clone)]
pub struct CitilinkConnector {
    client: reqwest::Client,
    solver: Option<Arc<NativeCdpSolver>>,
}

impl Default for CitilinkConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl CitilinkConnector {
    pub fn new() -> Self {
        Self::with_solver(None)
    }

    pub fn with_solver(solver: Option<Arc<NativeCdpSolver>>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
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

        Self { client, solver }
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
        let trimmed = content.trim();
        if (trimmed.starts_with('[') || trimmed.starts_with('{'))
            && let Ok(val) = serde_json::from_str::<Value>(trimmed)
        {
            return Self::parse_json_products(&val, limit);
        }

        let mut offers = Vec::new();

        // 1. Check window.__INITIAL_STATE__
        if let Some(script_data) =
            extract_between(content, "window.__INITIAL_STATE__ = ", ";</script>")
            && let Ok(val) = serde_json::from_str::<Value>(&script_data)
            && let Some(products) = val
                .get("catalog")
                .and_then(|c| c.get("products"))
                .or_else(|| val.get("products"))
            && let Ok(parsed) = Self::parse_json_products(products, limit)
        {
            offers.extend(parsed);
        }

        // 2. Check <script id="__NEXT_DATA__">
        if offers.is_empty()
            && let Some(next_data) = extract_between(
                content,
                "<script id=\"__NEXT_DATA__\" type=\"application/json\">",
                "</script>",
            )
            && let Ok(val) = serde_json::from_str::<Value>(&next_data)
            && let Some(products) = val
                .pointer("/props/pageProps/initialState/products")
                .or_else(|| val.pointer("/props/pageProps/products"))
                .or_else(|| val.pointer("/props/pageProps/catalog/products"))
            && let Ok(parsed) = Self::parse_json_products(products, limit)
        {
            offers.extend(parsed);
        }

        if !offers.is_empty() {
            return Ok(offers);
        }

        // 3. Modern HTML DOM parsing via scraper
        let document = Html::parse_document(content);
        let card_selector = Selector::parse(
            "[data-meta-name='ProductCard'], [data-meta-name='SnippetProduct'], [data-meta-name='ProductCardShort'], div[data-product-id]",
        )
        .unwrap();
        let title_selector = Selector::parse(
            "a[data-meta-name$='__title'], [data-meta-name='ProductCardTitle'], a.app-catalog-link, [title]",
        )
        .unwrap();
        let price_selector = Selector::parse(
            "[data-meta-name$='__price'], [data-meta-name='ProductPrice'], span[data-meta-price], .app-catalog-price",
        )
        .unwrap();
        let rating_selector = Selector::parse(
            "[data-meta-name$='__rating'], [data-meta-name='Rating'], [data-meta-rating]",
        )
        .unwrap();
        let reviews_selector =
            Selector::parse("[data-meta-name$='__reviews'], [data-meta-name='Reviews']").unwrap();

        for element in document.select(&card_selector) {
            if offers.len() >= limit {
                break;
            }

            // Title and URL
            let title_elem = element.select(&title_selector).next();
            let title = title_elem
                .and_then(|e| {
                    e.value()
                        .attr("title")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .or_else(|| {
                    title_elem
                        .map(|e| e.text().collect::<String>().trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_default();

            if title.is_empty() {
                continue;
            }

            let link = title_elem
                .and_then(|e| e.value().attr("href"))
                .unwrap_or_default();
            let full_url = if link.starts_with("http") {
                link.to_string()
            } else if !link.is_empty() {
                format!("https://www.citilink.ru{}", link)
            } else {
                String::new()
            };

            // Price
            let price_elem = element.select(&price_selector).next();
            let price = price_elem
                .and_then(|e| {
                    e.value()
                        .attr("data-meta-price")
                        .and_then(|p| p.parse::<u64>().ok())
                })
                .or_else(|| {
                    let text = price_elem
                        .map(|e| e.text().collect::<String>())
                        .unwrap_or_default();
                    Self::parse_price_str(&text)
                });

            let price = match price {
                Some(p) if p > 0 => p,
                _ => continue,
            };

            // Product ID
            let product_id = element
                .value()
                .attr("data-meta-product-id")
                .or_else(|| element.value().attr("data-product-id"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    // Try extracting id from URL: /product/...-(\d+)/
                    link.trim_end_matches('/')
                        .split('-')
                        .next_back()
                        .and_then(|last| {
                            last.chars()
                                .all(|c| c.is_ascii_digit())
                                .then(|| last.to_string())
                        })
                });

            // Rating
            let rating = element.select(&rating_selector).next().and_then(|e| {
                e.value()
                    .attr("data-meta-rating")
                    .and_then(|r| r.parse::<f32>().ok())
                    .or_else(|| {
                        let text = e.text().collect::<String>();
                        text.split_whitespace()
                            .next()
                            .and_then(|s| s.parse::<f32>().ok())
                    })
            });

            // Review count
            let review_count = element.select(&reviews_selector).next().and_then(|e| {
                let text = e.text().collect::<String>();
                let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
                digits.parse::<u32>().ok()
            });

            offers.push(ProductOffer {
                marketplace: Marketplace::Citilink,
                title,
                url: full_url,
                price_tier: PriceTier::Base(price),
                original_price: None,
                currency: Currency::Rub,
                rating,
                review_count,
                stock: StockStatus::InStock(1),
                seller: Some("Citilink".to_string()),
                product_id,
            });
        }

        if !offers.is_empty() {
            return Ok(offers);
        }

        // 4. Fallback to legacy data-params substring parsing
        let mut search_pos = 0;
        while let Some(start) = content[search_pos..].find("data-params='") {
            if offers.len() >= limit {
                break;
            }
            let abs_start = search_pos + start + "data-params='".len();
            if let Some(end) = content[abs_start..].find('\'') {
                let raw_json = &content[abs_start..abs_start + end];
                if let Ok(val) = serde_json::from_str::<Value>(raw_json) {
                    let name = val
                        .get("shortName")
                        .or_else(|| val.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let price = val.get("price").and_then(|v| v.as_u64()).unwrap_or(0);
                    let id = val.get("id").and_then(|v| v.as_str()).unwrap_or_default();

                    if !name.is_empty() && price > 0 {
                        offers.push(ProductOffer {
                            marketplace: Marketplace::Citilink,
                            title: name.to_string(),
                            url: format!("https://www.citilink.ru/product/{id}/"),
                            price_tier: PriceTier::Base(price),
                            original_price: None,
                            currency: Currency::Rub,
                            rating: None,
                            review_count: None,
                            stock: StockStatus::InStock(1),
                            seller: Some("Citilink".to_string()),
                            product_id: Some(id.to_string()),
                        });
                    }
                }
                search_pos = abs_start + end;
            } else {
                break;
            }
        }
        Ok(offers)
    }

    fn parse_json_products(val: &Value, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let items: Vec<&Value> = if let Some(arr) = val.as_array() {
            arr.iter().collect()
        } else if let Some(items) = val.get("items").and_then(|i| i.as_array()) {
            items.iter().collect()
        } else if let Some(products) = val.get("products").and_then(|p| p.as_array()) {
            products.iter().collect()
        } else {
            vec![val]
        };

        let mut offers = Vec::with_capacity(limit.min(items.len()));

        for item in items.iter().take(limit) {
            let name = item
                .get("name")
                .or_else(|| item.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim();

            if name.is_empty() {
                continue;
            }

            let price = item
                .get("price")
                .or_else(|| item.get("offers").and_then(|o| o.get("price")))
                .and_then(|v| match v {
                    Value::Number(n) => n.as_u64(),
                    Value::String(s) => Self::parse_price_str(s),
                    _ => None,
                })
                .unwrap_or(0);

            if price == 0 {
                continue;
            }

            let link = item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let full_url = if link.starts_with("http") {
                link.to_string()
            } else if !link.is_empty() {
                format!("https://www.citilink.ru{link}")
            } else {
                format!("https://www.citilink.ru/search/?text={name}")
            };

            offers.push(ProductOffer {
                marketplace: Marketplace::Citilink,
                title: clean_html(name),
                url: full_url,
                price_tier: PriceTier::Base(price),
                original_price: None,
                currency: Currency::Rub,
                rating: None,
                review_count: None,
                stock: StockStatus::InStock(1),
                seller: Some("Citilink".to_string()),
                product_id: item
                    .get("sku")
                    .or_else(|| item.get("id"))
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

        let mut url = reqwest::Url::parse(CITILINK_SEARCH_URL)
            .map_err(|e| ScraperError::Parse(e.to_string()))?;
        url.query_pairs_mut().append_pair("text", trimmed);
        let full_url = url.to_string();

        let body = match ru_marketplace_core::transport::send_with_transient_retry(
            2,
            std::time::Duration::from_millis(300),
            || self.client.get(&full_url).send(),
        )
        .await
        {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await?;
                if text.contains("qrator") || text.contains("challenge") {
                    if let Some(solver) = &self.solver {
                        info!(
                            "Citilink challenge detected, falling back to Chromium navigation..."
                        );
                        solver
                            .navigate_and_extract_html(Marketplace::Citilink, &full_url)
                            .await?
                    } else {
                        return Err(ScraperError::WafBlocked {
                            waf: WafProtection::Kasada,
                            details: "Citilink anti-bot challenge script detected".to_string(),
                        });
                    }
                } else {
                    text
                }
            }
            Ok(resp) => {
                if let Some(solver) = &self.solver {
                    info!(status = %resp.status(), "Citilink returned non-success, falling back to Chromium navigation...");
                    solver
                        .navigate_and_extract_html(Marketplace::Citilink, &full_url)
                        .await?
                } else {
                    return Err(ScraperError::WafBlocked {
                        waf: WafProtection::Kasada,
                        details: format!("Citilink HTTP {}", resp.status()),
                    });
                }
            }
            Err(e) => {
                if let Some(solver) = &self.solver {
                    info!(error = %e, "Citilink request error, falling back to Chromium navigation...");
                    solver
                        .navigate_and_extract_html(Marketplace::Citilink, &full_url)
                        .await?
                } else {
                    return Err(e);
                }
            }
        };

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
        let url = format!("https://www.citilink.ru/product/{id}/");
        let response = self.client.get(&url).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!("Citilink HTTP {status} (challenge active on card)"),
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
