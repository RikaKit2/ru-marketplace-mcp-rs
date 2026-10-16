use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use crate::connectors::{
    CitilinkConnector, DnsConnector, MarketplaceConnector, OzonConnector, WildberriesConnector,
};
use crate::domain::mcp::error_codes;
use crate::domain::{JsonRpcRequest, JsonRpcResponse, McpCallResult, McpTool};
use crate::services::AggregatorService;

/// MCP Stdio JSON-RPC 2.0 Server.
pub struct McpServer {
    aggregator: AggregatorService,
    wb: Arc<WildberriesConnector>,
    ozon: Arc<OzonConnector>,
    dns: Arc<DnsConnector>,
    citilink: Arc<CitilinkConnector>,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    pub fn new() -> Self {
        let session_manager = Arc::new(crate::session::SessionManager::default());
        let wb = Arc::new(WildberriesConnector::with_session_manager(
            "-1257786",
            session_manager.clone(),
        ));
        let ozon = Arc::new(OzonConnector::with_session_manager(session_manager.clone()));
        let dns = Arc::new(DnsConnector::default());
        let citilink = Arc::new(CitilinkConnector::default());

        let connectors: Vec<Arc<dyn MarketplaceConnector>> =
            vec![wb.clone(), ozon.clone(), dns.clone(), citilink.clone()];

        let aggregator = AggregatorService::new(connectors);

        Self {
            aggregator,
            wb,
            ozon,
            dns,
            citilink,
        }
    }

    /// Run the main Stdio loop processing newline-delimited JSON-RPC 2.0 messages.
    pub async fn run_stdio(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting ru-marketplace-mcp stdio transport...");

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                continue;
            }

            debug!("Received JSON-RPC message: {line_trimmed}");

            let request: JsonRpcRequest = match serde_json::from_str(line_trimmed) {
                Ok(req) => req,
                Err(err) => {
                    error!("JSON-RPC parse error: {err}");
                    let resp = JsonRpcResponse::error(
                        None,
                        error_codes::PARSE_ERROR,
                        format!("Parse error: {err}"),
                    );
                    let mut out = serde_json::to_string(&resp)?;
                    out.push('\n');
                    stdout.write_all(out.as_bytes()).await?;
                    stdout.flush().await?;
                    continue;
                }
            };

            // Notifications have no ID and expect no response
            if request.id.is_none() && request.method.starts_with("notifications/") {
                debug!("Handled notification: {}", request.method);
                continue;
            }

            let resp = self.handle_request(request).await;
            if let Some(resp) = resp {
                let mut out = serde_json::to_string(&resp)?;
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
            }
        }

        info!("ru-marketplace-mcp stdio transport terminated");
        Ok(())
    }

    /// Process a single incoming JSON-RPC 2.0 request.
    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id;
        let method = req.method.as_str();

        match method {
            "initialize" => {
                let result = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "ru-marketplace-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                Some(JsonRpcResponse::success(id, result))
            }
            "ping" => Some(JsonRpcResponse::success(id, json!({}))),
            "tools/list" => {
                let tools = self.list_tools();
                Some(JsonRpcResponse::success(id, json!({ "tools": tools })))
            }
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let arguments = params.get("arguments").unwrap_or(&Value::Null);

                let call_result = self.call_tool(tool_name, arguments).await;
                Some(JsonRpcResponse::success(
                    id,
                    serde_json::to_value(call_result).unwrap_or(Value::Null),
                ))
            }
            _ => {
                warn!("Unknown JSON-RPC method: {method}");
                Some(JsonRpcResponse::error(
                    id,
                    error_codes::METHOD_NOT_FOUND,
                    format!("Method '{method}' not found"),
                ))
            }
        }
    }

    /// List tools exposed by this MCP server.
    pub fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "compare_prices".to_string(),
                description: "Price one product across all Russian marketplaces (Wildberries, Ozon, DNS, Citilink) concurrently and rank by price.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search text, e.g. 'iphone 15 128' or 'стиральная машина узкая'"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to take per marketplace (default 5)",
                            "minimum": 1,
                            "maximum": 20
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "wb_search".to_string(),
                description: "Search Wildberries catalog using exact match v9 API with Moscow region pricing.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to return (default 10)",
                            "minimum": 1,
                            "maximum": 50
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "ozon_search".to_string(),
                description: "Search Ozon catalog via Composer API with WAF detection and handling.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to return (default 10)",
                            "minimum": 1,
                            "maximum": 50
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "dns_search".to_string(),
                description: "Search DNS-Shop catalog with anti-bot challenge detection.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to return (default 10)",
                            "minimum": 1,
                            "maximum": 50
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "citilink_search".to_string(),
                description: "Search Citilink catalog with anti-bot challenge detection.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to return (default 10)",
                            "minimum": 1,
                            "maximum": 50
                        }
                    },
                    "required": ["query"]
                }),
            },
        ]
    }

    /// Execute a tool by name with provided arguments.
    pub async fn call_tool(&self, name: &str, args: &Value) -> McpCallResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        if query.trim().is_empty() {
            return McpCallResult::error("Argument 'query' must not be empty");
        }

        match name {
            "compare_prices" => {
                let result = self.aggregator.compare_prices(query, limit, None).await;
                match serde_json::to_string_pretty(&result) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                }
            }
            "wb_search" => match self.wb.search(query, limit.max(10)).await {
                Ok(offers) => match serde_json::to_string_pretty(&offers) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                },
                Err(e) => McpCallResult::error(format!("Wildberries search error: {e}")),
            },
            "ozon_search" => match self.ozon.search(query, limit.max(10)).await {
                Ok(offers) => match serde_json::to_string_pretty(&offers) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                },
                Err(e) => McpCallResult::error(format!("Ozon search error: {e}")),
            },
            "dns_search" => match self.dns.search(query, limit.max(10)).await {
                Ok(offers) => match serde_json::to_string_pretty(&offers) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                },
                Err(e) => McpCallResult::error(format!("DNS search error: {e}")),
            },
            "citilink_search" => match self.citilink.search(query, limit.max(10)).await {
                Ok(offers) => match serde_json::to_string_pretty(&offers) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                },
                Err(e) => McpCallResult::error(format!("Citilink search error: {e}")),
            },
            unknown => McpCallResult::error(format!("Unknown tool: '{unknown}'")),
        }
    }
}
