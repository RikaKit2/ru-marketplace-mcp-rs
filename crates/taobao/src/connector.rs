use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{Marketplace, ProductOffer, ScraperError, SearchFilterOptions};
use ru_marketplace_solver::NativeCdpSolver;

use crate::models::{TaobaoCardDetails, TaobaoSearchItem};
use crate::parser::{
    canonical_item_url, canonical_search_url, extract_item_id, parse_card_html, parse_search_html,
};

/// Taobao marketplace connector backed by NativeCdpSolver for Chromium browser execution.
#[derive(Debug, Clone)]
pub struct TaobaoConnector {
    client: reqwest::Client,
    solver: Arc<NativeCdpSolver>,
}

impl Default for TaobaoConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl TaobaoConnector {
    pub fn new() -> Self {
        Self::with_solver(Arc::new(NativeCdpSolver::default()))
    }

    pub fn with_solver(solver: Arc<NativeCdpSolver>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, solver }
    }

    /// Access underlying solver instance.
    pub fn solver(&self) -> &Arc<NativeCdpSolver> {
        &self.solver
    }

    /// Access underlying HTTP client instance.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Execute search query returning native `TaobaoSearchItem`s with prices in CNY.
    pub async fn search_items(
        &self,
        query: &str,
        limit: usize,
        _filters: &SearchFilterOptions,
    ) -> Result<Vec<TaobaoSearchItem>, ScraperError> {
        let search_url = canonical_search_url(query, 1);
        info!("Navigating Taobao catalog search via CDP solver: {search_url}");

        let html = self
            .solver
            .navigate_and_extract_html(Marketplace::Taobao, &search_url)
            .await?;

        parse_search_html(&html, limit)
    }

    /// Fetch single item card details returning native `TaobaoCardDetails` with prices in CNY.
    pub async fn get_card_details(&self, item_id: &str) -> Result<TaobaoCardDetails, ScraperError> {
        let id = extract_item_id(item_id).ok_or_else(|| {
            ScraperError::InvalidFilter(format!(
                "Invalid Taobao item ID or URL: '{item_id}'. Expected 9-13 digit numeric ID or taobao.com URL."
            ))
        })?;

        let card_url = canonical_item_url(&id);
        info!("Fetching Taobao product card via CDP solver: {card_url}");

        let html = self
            .solver
            .navigate_and_extract_html(Marketplace::Taobao, &card_url)
            .await?;

        parse_card_html(&html, Some(&id))
    }
}

#[async_trait]
impl MarketplaceConnector for TaobaoConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Taobao
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let items = self.search_items(query, limit, filters).await?;
        let offers: Vec<ProductOffer> = items.into_iter().map(ProductOffer::from).collect();
        Ok(offers)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let card = self.get_card_details(item_id).await?;
        Ok(ProductOffer::from(card))
    }
}
