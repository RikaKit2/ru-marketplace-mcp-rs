pub mod connectors;
pub mod domain;
pub mod mcp;
pub mod services;

pub use connectors::{
    CitilinkConnector, DnsConnector, MarketplaceConnector, OzonConnector, WildberriesConnector,
};
pub use domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
    SubscriptionType, WafProtection,
};
pub use mcp::McpServer;
pub use services::{AggregatorService, CompareResult, SourceOutcome};
