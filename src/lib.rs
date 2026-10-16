pub mod connectors;
pub mod domain;
pub mod mcp;
pub mod services;
pub mod session;
pub mod transport;

pub use connectors::{
    CitilinkConnector, DnsConnector, MarketplaceConnector, OzonConnector, WildberriesConnector,
};
pub use domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
    SubscriptionType, WafProtection,
};
pub use mcp::McpServer;
pub use services::{AggregatorService, CompareResult, SourceOutcome};
pub use session::{BunSolver, CookieStore, SessionData, SessionManager};
pub use transport::{apply_browser_fingerprint, build_browser_headers};
