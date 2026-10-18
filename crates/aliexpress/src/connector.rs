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

use crate::models::{AliExpressCardResponse, AliExpressSearchResponse};
use crate::parser::{
    card_url, extract_item_id, is_empty_shell, is_punish, parse_card_page, parse_search_page,
    search_url,
};

/// AliExpress marketplace connector providing search and product card retrieval.
#[derive(Debug, Clone)]
pub struct AliExpressConnector {
    client: reqwest::Client,
    solver: Option<Arc<NativeCdpSolver>>,
}

impl Default for AliExpressConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl AliExpressConnector {
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
        headers.insert(REFERER, HeaderValue::from_static("https://aliexpress.ru/"));

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

    pub fn solver(&self) -> Option<&Arc<NativeCdpSolver>> {
        self.solver.as_ref()
    }

    /// Fetch HTML for an AliExpress page via direct HTTP, falling back to NativeCdpSolver
    /// if an x5sec challenge, empty client SPA shell, or refusal status occurs.
    pub async fn fetch_html(&self, url: &str) -> Result<String, ScraperError> {
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
                        info!(status = %status, "AliExpress HTTP refusal, falling back to Chromium CDP...");
                        return solver
                            .navigate_and_extract_html(Marketplace::AliExpress, url)
                            .await;
                    } else {
                        return Err(ScraperError::WafBlocked {
                            waf: WafProtection::Kasada,
                            details: format!("AliExpress HTTP {status} (x5sec challenge active)"),
                        });
                    }
                }

                let body = resp.text().await?;
                if is_punish(&body) || is_empty_shell(&body) {
                    if let Some(solver) = &self.solver {
                        info!(
                            "AliExpress x5sec challenge or empty SPA shell detected, falling back to Chromium CDP..."
                        );
                        return solver
                            .navigate_and_extract_html(Marketplace::AliExpress, url)
                            .await;
                    } else if is_punish(&body) {
                        return Err(ScraperError::WafBlocked {
                            waf: WafProtection::Kasada,
                            details: "AliExpress x5sec anti-bot challenge detected (browser solver required)"
                                .to_string(),
                        });
                    } else {
                        return Err(ScraperError::Parse(
                            "AliExpress returned empty client-rendered SPA shell (browser solver required)"
                                .to_string(),
                        ));
                    }
                }
                Ok(body)
            }
            Err(e) => {
                if let Some(solver) = &self.solver {
                    info!(error = %e, "AliExpress network error, falling back to Chromium CDP...");
                    solver
                        .navigate_and_extract_html(Marketplace::AliExpress, url)
                        .await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Search AliExpress catalog returning rich domain items.
    pub async fn search_aliexpress(
        &self,
        query: &str,
        page: usize,
        limit: usize,
    ) -> Result<AliExpressSearchResponse, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(AliExpressSearchResponse {
                status: "success".to_string(),
                query: String::new(),
                count: 0,
                items: Vec::new(),
            });
        }

        let url = search_url(trimmed, page);
        let html = self.fetch_html(&url).await?;
        let items = parse_search_page(&html, limit)?;

        Ok(AliExpressSearchResponse {
            status: "success".to_string(),
            query: trimmed.to_string(),
            count: items.len(),
            items,
        })
    }

    /// Get product card details by bare numeric ID or AliExpress URL.
    pub async fn get_card_aliexpress(
        &self,
        item_id_or_url: &str,
    ) -> Result<AliExpressCardResponse, ScraperError> {
        let trimmed = item_id_or_url.trim();
        let Some(item_id) = extract_item_id(trimmed) else {
            return Err(ScraperError::InvalidFilter(format!(
                "Invalid AliExpress item ID or URL: '{item_id_or_url}'. Expected 9-16 digit product ID."
            )));
        };

        let url = card_url(&item_id);
        let html = self.fetch_html(&url).await?;
        let card = parse_card_page(&html, &item_id)?;

        Ok(AliExpressCardResponse {
            status: "success".to_string(),
            item_id,
            card,
        })
    }
}

#[async_trait]
impl MarketplaceConnector for AliExpressConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::AliExpress
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let search_resp = self.search_aliexpress(query, 1, limit).await?;
        let mut offers: Vec<ProductOffer> = search_resp.items.into_iter().map(Into::into).collect();

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

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let card_resp = self.get_card_aliexpress(item_id).await?;
        Ok(card_resp.card.into())
    }
}
