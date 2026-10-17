use async_trait::async_trait;

use crate::domain::{Marketplace, ProductOffer, ScraperError, SearchFilterOptions};

/// Common asynchronous interface implemented by every marketplace connector.
#[async_trait]
pub trait MarketplaceConnector: Send + Sync {
    /// Return the marketplace handled by this connector.
    fn marketplace(&self) -> Marketplace;

    /// Execute text search on the marketplace catalog with optional filters and sorting.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError>;

    /// Fetch a single detailed product card by native product/SKU ID.
    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError>;
}
