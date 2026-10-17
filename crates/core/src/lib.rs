pub mod connector;
pub mod domain;
pub mod services;
pub mod session;
pub mod transport;

pub use connector as connectors;
pub use connector::MarketplaceConnector;
pub use domain::{
    CardType, Currency, JsonRpcError, JsonRpcRequest, JsonRpcResponse, Marketplace, McpCallResult,
    McpTextContent, McpTool, McpToolRequest, PriceTier, ProductOffer, ScraperError,
    SearchFilterOptions, SortOrder, StockStatus, SubscriptionType, WafProtection, error_codes,
};
pub use services::{AggregatorService, CompareResult, SourceOutcome};
pub use session::{CookieStore, SessionData, SessionManager, SessionSolver, SessionStatusInfo};
pub use transport::{
    ACCEPT_LANG_RU, DEFAULT_DESKTOP_UA, SEC_CH_UA, SEC_CH_UA_MOBILE, SEC_CH_UA_PLATFORM,
    apply_browser_fingerprint, build_browser_headers,
};
