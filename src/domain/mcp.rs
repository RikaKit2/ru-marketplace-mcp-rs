use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::marketplaces::Marketplace;

/// Sorting criteria for marketplace search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Popular,
    PriceAsc,
    PriceDesc,
    Rating,
    Reviews,
    Discount,
}

impl std::str::FromStr for SortOrder {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "popular" => Ok(Self::Popular),
            "price_asc" | "price" | "priceup" | "cheap" => Ok(Self::PriceAsc),
            "price_desc" | "pricedown" | "expensive" => Ok(Self::PriceDesc),
            "rating" | "rate" => Ok(Self::Rating),
            "reviews" | "review_count" | "feedback" => Ok(Self::Reviews),
            "discount" | "benefit" => Ok(Self::Discount),
            _ => Err(()),
        }
    }
}

/// Filter and sorting parameters applied to catalog searches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilterOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_price: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_price: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortOrder>,
}

/// High-level MCP tool request variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", content = "params", rename_all = "snake_case")]
pub enum McpToolRequest {
    ComparePrices {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filters: Option<SearchFilterOptions>,
    },
    Search {
        marketplace: Marketplace,
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filters: Option<SearchFilterOptions>,
    },
    GetCard {
        marketplace: Marketplace,
        item_id: String,
    },
}

/// Incoming JSON-RPC 2.0 request or notification envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Construct a successful response.
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error response.
    pub fn error(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC 2.0 error representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// MCP tool descriptor for tools/list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP tool execution text content result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTextContent {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

impl McpTextContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text".to_string(),
            text: text.into(),
        }
    }
}

/// Result payload for tools/call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpCallResult {
    pub content: Vec<McpTextContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl McpCallResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![McpTextContent::text(text)],
            is_error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![McpTextContent::text(message)],
            is_error: Some(true),
        }
    }
}
