pub mod errors;
pub mod marketplaces;
pub mod mcp;
pub mod prices;

pub use crate::services::verification::{OfferVerificationResult, verify_offer};
pub use errors::ScraperError;
pub use marketplaces::{
    CardType, Currency, Marketplace, StockStatus, SubscriptionType, WafProtection,
};
pub use mcp::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpCallResult, McpTextContent, McpTool,
    McpToolRequest, SearchFilterOptions, SortOrder, error_codes,
};
pub use prices::{PriceTier, ProductOffer};
