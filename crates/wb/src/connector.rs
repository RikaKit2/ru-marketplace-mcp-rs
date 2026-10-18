use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{
    Marketplace, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
};
use ru_marketplace_core::session::SessionManager;
use ru_marketplace_solver::NativeCdpSolver;

use super::basket::BasketResolver;
use super::parser::parse_products_json;

pub const WB_DEFAULT_DEST: &str = "1259570991";
pub const WB_SEARCH_URL: &str =
    "https://www.wildberries.ru/__internal/u-search/exactmatch/ru/common/v18/search";
pub const WB_CARD_URL: &str = "https://card.wb.ru/cards/v4/detail";

/// Wildberries marketplace catalog connector with CDN shard resolving and solver search.
#[derive(Debug, Clone)]
pub struct WildberriesConnector {
    dest: String,
    basket_resolver: BasketResolver,
    session_manager: Arc<SessionManager>,
    solver: Arc<NativeCdpSolver>,
}

impl Default for WildberriesConnector {
    fn default() -> Self {
        Self::new(WB_DEFAULT_DEST)
    }
}

impl WildberriesConnector {
    pub fn new(dest: impl Into<String>) -> Self {
        let solver = Arc::new(NativeCdpSolver::default());
        let session_manager = Arc::new(SessionManager::default());
        Self::with_components(dest, session_manager, solver)
    }

    pub fn with_components(
        dest: impl Into<String>,
        session_manager: Arc<SessionManager>,
        solver: Arc<NativeCdpSolver>,
    ) -> Self {
        Self {
            dest: dest.into(),
            basket_resolver: BasketResolver::default(),
            session_manager,
            solver,
        }
    }

    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
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

        let mut url =
            reqwest::Url::parse(WB_SEARCH_URL).map_err(|e| ScraperError::Parse(e.to_string()))?;
        for (k, v) in query_params {
            url.query_pairs_mut().append_pair(k, &v);
        }
        let search_url = url.to_string();

        // Execute search strictly via browser tab to ensure genuine Chrome TLS Client Hello
        let body = self
            .solver
            .fetch_in_tab(Marketplace::Wildberries, &search_url)
            .await?;

        let mut offers = parse_products_json(&body, limit)?;

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
        let id_clean = item_id.trim();
        let nm = id_clean
            .parse::<u64>()
            .map_err(|_| ScraperError::NotFound {
                query: id_clean.to_string(),
            })?;

        // 1. Primary fast-path: fetch directly from CDN basket shard (50ms, zero browser)
        match self.basket_resolver.get_card(nm).await {
            Ok(card) => return Ok(card),
            Err(err) => {
                info!(
                    "CDN basket lookup missed for nm {nm}: {err}, falling back to v4 card detail via browser..."
                );
            }
        }

        // 2. Fallback: v4 card API through solver tab
        let card_url = format!(
            "{}?appType=1&curr=rub&dest={}&spp=30&nm={nm}",
            WB_CARD_URL, self.dest
        );

        let body = self
            .solver
            .fetch_in_tab(Marketplace::Wildberries, &card_url)
            .await?;

        let offers = parse_products_json(&body, 1)?;
        offers
            .into_iter()
            .next()
            .ok_or_else(|| ScraperError::NotFound {
                query: id_clean.to_string(),
            })
    }
}
