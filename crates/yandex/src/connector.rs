use async_trait::async_trait;
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderName, HeaderValue, REFERER, USER_AGENT,
};
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{
    Marketplace, ProductOffer, ScraperError, SearchFilterOptions, SortOrder, WafProtection,
};
use ru_marketplace_core::transport::{
    ACCEPT_LANG_RU, DEFAULT_DESKTOP_UA, SEC_CH_UA, SEC_CH_UA_MOBILE, SEC_CH_UA_PLATFORM,
};
use ru_marketplace_solver::NativeCdpSolver;

use crate::models::{ParseStatus, YandexCardDetails, YandexSearchResult};
use crate::ssr::{card_url, looks_like_captcha, parse_card, parse_search};

pub const YANDEX_SEARCH_URL: &str = "https://market.yandex.ru/search";
pub const YANDEX_BASE_URL: &str = "https://market.yandex.ru";

/// Yandex Market connector reading server-rendered state and widget payloads.
#[derive(Debug, Clone)]
pub struct YandexConnector {
    client: reqwest::Client,
    solver: Option<Arc<NativeCdpSolver>>,
}

impl Default for YandexConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl YandexConnector {
    pub fn new() -> Self {
        Self::with_solver(None)
    }

    pub fn with_solver(solver: Option<Arc<NativeCdpSolver>>) -> Self {
        let mut headers = HeaderMap::new();

        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_DESKTOP_UA));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static(ACCEPT_LANG_RU));

        headers.insert(
            HeaderName::from_static("sec-ch-ua"),
            HeaderValue::from_static(SEC_CH_UA),
        );
        headers.insert(
            HeaderName::from_static("sec-ch-ua-mobile"),
            HeaderValue::from_static(SEC_CH_UA_MOBILE),
        );
        headers.insert(
            HeaderName::from_static("sec-ch-ua-platform"),
            HeaderValue::from_static(SEC_CH_UA_PLATFORM),
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-mode"),
            HeaderValue::from_static("navigate"),
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-site"),
            HeaderValue::from_static("same-origin"),
        );
        headers.insert(
            HeaderName::from_static("sec-fetch-user"),
            HeaderValue::from_static("?1"),
        );
        headers.insert(
            HeaderName::from_static("upgrade-insecure-requests"),
            HeaderValue::from_static("1"),
        );
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://market.yandex.ru/"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, solver }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Parse HTML of a search page directly into normalized ProductOffers.
    pub fn parse_search_page(html: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        let result = parse_search(html);
        if result.status == ParseStatus::Captcha {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "Yandex SmartCaptcha detected in search HTML".to_string(),
            });
        }
        let mut offers: Vec<ProductOffer> = result.items.into_iter().map(Into::into).collect();
        if offers.len() > limit {
            offers.truncate(limit);
        }
        Ok(offers)
    }

    /// Parse HTML of a product card page directly into a normalized ProductOffer.
    pub fn parse_card_page(html: &str) -> Result<ProductOffer, ScraperError> {
        let card = parse_card(html);
        if card.status == ParseStatus::Captcha {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "Yandex SmartCaptcha detected in card HTML".to_string(),
            });
        }
        if card.status == ParseStatus::EmptyProductShell {
            return Err(ScraperError::NotFound {
                query: "empty_product_shell: degraded serving or delisted product".to_string(),
            });
        }
        if card.title.is_empty() && card.price_rub.is_none() && card.price_with_plus.is_none() {
            return Err(ScraperError::NotFound {
                query: "no product details found on page".to_string(),
            });
        }
        Ok(ProductOffer::from(card))
    }

    /// Fetch HTML with transient retry, status code checks, and optional CDP solver fallback.
    async fn fetch_html(&self, url: &str) -> Result<String, ScraperError> {
        let resp_result = ru_marketplace_core::transport::send_with_transient_retry(
            2,
            std::time::Duration::from_millis(300),
            || self.client.get(url).send(),
        )
        .await;

        match resp_result {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return Err(ScraperError::NotFound {
                        query: url.to_string(),
                    });
                }
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Err(ScraperError::RateLimited {
                        retry_after_secs: None,
                    });
                }
                if status == reqwest::StatusCode::FORBIDDEN
                    || status == reqwest::StatusCode::UNAUTHORIZED
                {
                    if let Some(solver) = &self.solver {
                        info!(status = %status, "Yandex HTTP refusal, falling back to Chromium CDP...");
                        return solver
                            .navigate_and_extract_html(Marketplace::YandexMarket, url)
                            .await;
                    } else {
                        return Err(ScraperError::WafBlocked {
                            waf: WafProtection::Kasada,
                            details: format!("Yandex HTTP {status}"),
                        });
                    }
                }
                let body = resp.text().await?;
                if looks_like_captcha(&body)
                    && let Some(solver) = &self.solver
                {
                    info!(
                        "Yandex SmartCaptcha detected in response body, falling back to Chromium CDP..."
                    );
                    return solver
                        .navigate_and_extract_html(Marketplace::YandexMarket, url)
                        .await;
                }
                Ok(body)
            }
            Err(e) => {
                if let Some(solver) = &self.solver {
                    info!(error = %e, "Yandex transport error, falling back to Chromium CDP...");
                    solver
                        .navigate_and_extract_html(Marketplace::YandexMarket, url)
                        .await
                } else {
                    Err(e)
                }
            }
        }
    }

    fn process_search_result(
        &self,
        parsed: YandexSearchResult,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let mut offers: Vec<ProductOffer> = parsed.items.into_iter().map(Into::into).collect();

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

        if offers.len() > limit {
            offers.truncate(limit);
        }

        Ok(offers)
    }

    /// Search Yandex Market returning rich domain result.
    pub async fn search_yandex(
        &self,
        query: &str,
        page: u32,
        _limit: usize,
    ) -> Result<YandexSearchResult, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(YandexSearchResult {
                status: ParseStatus::Empty,
                query: String::new(),
                total: None,
                page: Some(page),
                page_count: None,
                has_next_page: false,
                items: Vec::new(),
            });
        }

        let mut url = reqwest::Url::parse(YANDEX_SEARCH_URL)
            .map_err(|e| ScraperError::Parse(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("text", trimmed)
            .append_pair("page", &page.to_string());

        let full_url = url.to_string();
        let body = self.fetch_html(&full_url).await?;
        let parsed = parse_search(&body);

        if parsed.status == ParseStatus::Captcha {
            if let Some(solver) = &self.solver {
                info!("Yandex SmartCaptcha in search_yandex, falling back to Chromium CDP...");
                let solver_html = solver
                    .navigate_and_extract_html(Marketplace::YandexMarket, &full_url)
                    .await?;
                return Ok(parse_search(&solver_html));
            } else {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Kasada,
                    details: "Yandex SmartCaptcha challenge detected during search".to_string(),
                });
            }
        }

        Ok(parsed)
    }

    /// Fetch rich card details including rating breakdown and embedded reviews.
    pub async fn get_card_details(&self, item_id: &str) -> Result<YandexCardDetails, ScraperError> {
        let trimmed = item_id.trim();
        if trimmed.is_empty() {
            return Err(ScraperError::NotFound {
                query: item_id.to_string(),
            });
        }

        let full_url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with("product--") {
            format!("{YANDEX_BASE_URL}/{trimmed}")
        } else if trimmed.starts_with("/card/") || trimmed.starts_with("/product/") {
            format!("{YANDEX_BASE_URL}{trimmed}")
        } else {
            card_url(trimmed, None)
        };

        let body = self.fetch_html(&full_url).await?;
        let parsed = parse_card(&body);

        if parsed.status == ParseStatus::Captcha {
            if let Some(solver) = &self.solver {
                info!("Yandex SmartCaptcha in get_card_details, falling back to Chromium CDP...");
                let solver_html = solver
                    .navigate_and_extract_html(Marketplace::YandexMarket, &full_url)
                    .await?;
                let re_parsed = parse_card(&solver_html);
                if re_parsed.status == ParseStatus::Captcha {
                    return Err(ScraperError::WafBlocked {
                        waf: WafProtection::Kasada,
                        details: "Yandex SmartCaptcha challenge persists on card".to_string(),
                    });
                }
                return Ok(re_parsed);
            } else {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Kasada,
                    details: "Yandex SmartCaptcha challenge detected on card".to_string(),
                });
            }
        }

        Ok(parsed)
    }
}

#[async_trait]
impl MarketplaceConnector for YandexConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::YandexMarket
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

        let mut url = reqwest::Url::parse(YANDEX_SEARCH_URL)
            .map_err(|e| ScraperError::Parse(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("text", trimmed)
            .append_pair("page", "1");
        let full_url = url.to_string();

        let body = self.fetch_html(&full_url).await?;
        let parsed = parse_search(&body);

        if parsed.status == ParseStatus::Captcha {
            if let Some(solver) = &self.solver {
                info!("Yandex SmartCaptcha detected in search, falling back to Chromium CDP...");
                let solver_html = solver
                    .navigate_and_extract_html(Marketplace::YandexMarket, &full_url)
                    .await?;
                let re_parsed = parse_search(&solver_html);
                return self.process_search_result(re_parsed, limit, filters);
            } else {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Kasada,
                    details: "Yandex SmartCaptcha challenge detected during search".to_string(),
                });
            }
        }

        self.process_search_result(parsed, limit, filters)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let trimmed = item_id.trim();
        if trimmed.is_empty() {
            return Err(ScraperError::NotFound {
                query: item_id.to_string(),
            });
        }

        let full_url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with("product--") {
            format!("{YANDEX_BASE_URL}/{trimmed}")
        } else if trimmed.starts_with("/card/") || trimmed.starts_with("/product/") {
            format!("{YANDEX_BASE_URL}{trimmed}")
        } else {
            card_url(trimmed, None)
        };

        let body = self.fetch_html(&full_url).await?;
        let parsed = parse_card(&body);

        if parsed.status == ParseStatus::Captcha {
            if let Some(solver) = &self.solver {
                info!(
                    "Yandex SmartCaptcha detected on product card, falling back to Chromium CDP..."
                );
                let solver_html = solver
                    .navigate_and_extract_html(Marketplace::YandexMarket, &full_url)
                    .await?;
                let re_parsed = parse_card(&solver_html);
                if re_parsed.status == ParseStatus::Captcha {
                    return Err(ScraperError::WafBlocked {
                        waf: WafProtection::Kasada,
                        details: "Yandex SmartCaptcha challenge persists on card".to_string(),
                    });
                }
                return self.card_to_offer(re_parsed, trimmed);
            } else {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Kasada,
                    details: "Yandex SmartCaptcha challenge detected on product card".to_string(),
                });
            }
        }

        self.card_to_offer(parsed, trimmed)
    }
}

impl YandexConnector {
    fn card_to_offer(
        &self,
        card: YandexCardDetails,
        original_id: &str,
    ) -> Result<ProductOffer, ScraperError> {
        if card.status == ParseStatus::EmptyProductShell {
            return Err(ScraperError::NotFound {
                query: format!("{original_id} (empty product shell / delisted)"),
            });
        }

        if card.title.is_empty() && card.price_rub.is_none() && card.price_with_plus.is_none() {
            return Err(ScraperError::NotFound {
                query: original_id.to_string(),
            });
        }

        Ok(ProductOffer::from(card))
    }
}
