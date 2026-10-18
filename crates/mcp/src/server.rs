use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use ru_marketplace_citilink::CitilinkConnector;
use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::error_codes;
use ru_marketplace_core::domain::{
    JsonRpcRequest, JsonRpcResponse, Marketplace, McpCallResult, McpTool, SearchFilterOptions,
    SortOrder,
};
use ru_marketplace_core::services::AggregatorService;
use ru_marketplace_core::session::SessionManager;
use ru_marketplace_dns::DnsConnector;
use ru_marketplace_ozon::OzonConnector;
use ru_marketplace_solver::NativeCdpSolver;
use ru_marketplace_wb::{WB_DEFAULT_DEST, WildberriesConnector};

/// MCP Stdio JSON-RPC 2.0 Server.
pub struct McpServer {
    aggregator: AggregatorService,
    wb: Arc<WildberriesConnector>,
    ozon: Arc<OzonConnector>,
    dns: Arc<DnsConnector>,
    citilink: Arc<CitilinkConnector>,
    session_manager: Arc<SessionManager>,
    #[allow(dead_code)]
    solver: Arc<NativeCdpSolver>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let sm = self.session_manager.clone();
        tokio::spawn(async move {
            let _ = sm.stop_session("all").await;
        });
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    pub fn new() -> Self {
        let session_manager = Arc::new(SessionManager::default());
        let solver = Arc::new(NativeCdpSolver::default());

        // Register multi-tab solvers for Ozon and Wildberries
        session_manager.register_solver_sync(solver.as_session_solver(Marketplace::Ozon));
        session_manager.register_solver_sync(solver.as_session_solver(Marketplace::Wildberries));

        let wb = Arc::new(WildberriesConnector::with_components(
            WB_DEFAULT_DEST,
            session_manager.clone(),
            solver.clone(),
        ));
        let ozon = Arc::new(OzonConnector::with_solver(
            session_manager.clone(),
            solver.clone(),
        ));
        let dns = Arc::new(DnsConnector::with_solver(Some(solver.clone())));
        let citilink = Arc::new(CitilinkConnector::with_solver(Some(solver.clone())));

        let connectors: Vec<Arc<dyn MarketplaceConnector>> =
            vec![wb.clone(), ozon.clone(), dns.clone(), citilink.clone()];

        let aggregator = AggregatorService::new(connectors);

        Self {
            aggregator,
            wb,
            ozon,
            dns,
            citilink,
            session_manager,
            solver,
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

        info!("ru-marketplace-mcp stdio transport received EOF, cleaning up active sessions...");
        let _ = self.session_manager.stop_session("all").await;
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
                warn!("Unknown JSON-RPC method requested: {method}");
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
                        "marketplace": {
                            "type": "string",
                            "description": "Optional single marketplace filter (wildberries, ozon, dns, citilink)"
                        },
                        "marketplaces": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional list of marketplaces to search (e.g. ['wildberries', 'ozon'])"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum offers to take per marketplace (default 5)",
                            "minimum": 1,
                            "maximum": 20
                        },
                        "min_price": {
                            "type": "integer",
                            "description": "Minimum price in rubles",
                            "minimum": 0
                        },
                        "max_price": {
                            "type": "integer",
                            "description": "Maximum price in rubles",
                            "minimum": 0
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria",
                            "enum": ["popular", "price_asc", "price_desc", "rating", "reviews", "discount"]
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "wb_search".to_string(),
                description: "Search Wildberries catalog using authentic browser TLS handshake (bypassing wbaas WAF).".to_string(),
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
                        },
                        "min_price": {
                            "type": "integer",
                            "description": "Minimum price in rubles",
                            "minimum": 0
                        },
                        "max_price": {
                            "type": "integer",
                            "description": "Maximum price in rubles",
                            "minimum": 0
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria",
                            "enum": ["popular", "price_asc", "price_desc", "rating", "reviews", "discount"]
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "wb_card".to_string(),
                description: "Get detailed product card information for a Wildberries item by article (nm ID) directly via CDN shards (~50ms latency).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "Wildberries article / nm ID, e.g. '164379765'"
                        }
                    },
                    "required": ["item_id"]
                }),
            },
            McpTool {
                name: "ozon_search".to_string(),
                description: "Search Ozon catalog via Composer API with Variti PoW solver handling.".to_string(),
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
                        },
                        "min_price": {
                            "type": "integer",
                            "description": "Minimum price in rubles",
                            "minimum": 0
                        },
                        "max_price": {
                            "type": "integer",
                            "description": "Maximum price in rubles",
                            "minimum": 0
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria",
                            "enum": ["popular", "price_asc", "price_desc", "rating", "reviews", "discount"]
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "ozon_card".to_string(),
                description: "Get detailed product card information for an Ozon item by SKU or URL.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "Ozon product SKU or ID, e.g. '3087570418'"
                        }
                    },
                    "required": ["item_id"]
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
                        },
                        "min_price": {
                            "type": "integer",
                            "description": "Minimum price in rubles",
                            "minimum": 0
                        },
                        "max_price": {
                            "type": "integer",
                            "description": "Maximum price in rubles",
                            "minimum": 0
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria",
                            "enum": ["popular", "price_asc", "price_desc", "rating", "reviews", "discount"]
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
                        },
                        "min_price": {
                            "type": "integer",
                            "description": "Minimum price in rubles",
                            "minimum": 0
                        },
                        "max_price": {
                            "type": "integer",
                            "description": "Maximum price in rubles",
                            "minimum": 0
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria",
                            "enum": ["popular", "price_asc", "price_desc", "rating", "reviews", "discount"]
                        }
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "session_start".to_string(),
                description: "Start and pre-warm a unified background Chromium browser session on Xvfb :99 for Ozon and Wildberries (~250ms execution). Closes automatically after 5 minutes of idle time.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "marketplace": {
                            "type": "string",
                            "description": "Target marketplace: 'all', 'ozon', 'wb', 'dns', or 'citilink' (default 'all')",
                            "enum": ["all", "ozon", "wb", "dns", "citilink"]
                        }
                    }
                }),
            },
            McpTool {
                name: "session_stop".to_string(),
                description: "Explicitly terminate active background browser sessions and free memory immediately.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "marketplace": {
                            "type": "string",
                            "description": "Target marketplace: 'all', 'ozon', 'wb', 'dns', or 'citilink' (default 'all')",
                            "enum": ["all", "ozon", "wb", "dns", "citilink"]
                        }
                    }
                }),
            },
            McpTool {
                name: "session_status".to_string(),
                description: "Inspect the status of active background browser solvers, cookie cache, and token expirations.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "marketplace": {
                            "type": "string",
                            "description": "Target marketplace: 'all', 'ozon', 'wb', 'dns', or 'citilink' (default 'all')",
                            "enum": ["all", "ozon", "wb", "dns", "citilink"]
                        }
                    }
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

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|l| l as usize)
            .unwrap_or(5);

        let min_price = args.get("min_price").and_then(|v| v.as_u64());
        let max_price = args.get("max_price").and_then(|v| v.as_u64());
        let sort = args
            .get("sort")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "price_asc" => Some(SortOrder::PriceAsc),
                "price_desc" => Some(SortOrder::PriceDesc),
                "rating" => Some(SortOrder::Rating),
                "reviews" => Some(SortOrder::Reviews),
                "discount" => Some(SortOrder::Discount),
                "popular" => Some(SortOrder::Popular),
                _ => None,
            });

        let filters = SearchFilterOptions {
            min_price,
            max_price,
            sort,
        };

        match name {
            "compare_prices" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                let filter_sources = if let Some(m_str) = args
                    .get("marketplace")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    match Marketplace::from_str(m_str) {
                        Ok(m) => Some(vec![m]),
                        Err(e) => return McpCallResult::error(format!("Invalid marketplace: {e}")),
                    }
                } else if let Some(arr) = args.get("marketplaces").and_then(|v| v.as_array()) {
                    let mut list = Vec::new();
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            match Marketplace::from_str(s) {
                                Ok(m) => list.push(m),
                                Err(e) => {
                                    return McpCallResult::error(format!(
                                        "Invalid marketplace: {e}"
                                    ));
                                }
                            }
                        }
                    }
                    if list.is_empty() { None } else { Some(list) }
                } else {
                    None
                };

                let result = self
                    .aggregator
                    .compare_prices(query, limit, filter_sources.as_deref(), Some(&filters))
                    .await;
                match serde_json::to_string_pretty(&result) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                }
            }
            "wb_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.wb.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries search error: {e}")),
                }
            }
            "wb_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.wb.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries card error: {e}")),
                }
            }
            "ozon_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.ozon.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Ozon search error: {e}")),
                }
            }
            "ozon_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.ozon.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Ozon card error: {e}")),
                }
            }
            "dns_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.dns.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("DNS search error: {e}")),
                }
            }
            "citilink_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.citilink.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Citilink search error: {e}")),
                }
            }
            "session_start" => {
                let target = args
                    .get("marketplace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("all");
                match self.session_manager.start_session(target).await {
                    Ok(msg) => McpCallResult::text(msg),
                    Err(e) => {
                        McpCallResult::error(format!("Failed to start session for '{target}': {e}"))
                    }
                }
            }
            "session_stop" => {
                let target = args
                    .get("marketplace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("all");
                match self.session_manager.stop_session(target).await {
                    Ok(msg) => McpCallResult::text(msg),
                    Err(e) => {
                        McpCallResult::error(format!("Failed to stop session for '{target}': {e}"))
                    }
                }
            }
            "session_status" => {
                let target = args
                    .get("marketplace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("all");
                let statuses = self.session_manager.session_status(target).await;
                match serde_json::to_string_pretty(&statuses) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                }
            }
            // Backward-compatibility aliases
            "ozon_session_start" => match self.session_manager.start_session("ozon").await {
                Ok(msg) => McpCallResult::text(msg),
                Err(e) => McpCallResult::error(format!("Failed to start Ozon session: {e}")),
            },
            "ozon_session_stop" => match self.session_manager.stop_session("ozon").await {
                Ok(msg) => McpCallResult::text(msg),
                Err(e) => McpCallResult::error(format!("Failed to stop Ozon session: {e}")),
            },
            unknown => McpCallResult::error(format!("Unknown tool: '{unknown}'")),
        }
    }
}
