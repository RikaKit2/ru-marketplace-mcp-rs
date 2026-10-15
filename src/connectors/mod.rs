use async_trait::async_trait;

use crate::domain::{Marketplace, ProductOffer, ScraperError};

pub mod citilink;
pub mod dns;
pub mod ozon;
pub mod wildberries;

pub use citilink::CitilinkConnector;
pub use dns::DnsConnector;
pub use ozon::OzonConnector;
pub use wildberries::WildberriesConnector;

/// Common asynchronous interface implemented by every marketplace connector.
#[async_trait]
pub trait MarketplaceConnector: Send + Sync {
    /// Return the marketplace handled by this connector.
    fn marketplace(&self) -> Marketplace;

    /// Execute text search on the marketplace catalog.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError>;

    /// Fetch a single detailed product card by native product/SKU ID.
    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError>;
}
