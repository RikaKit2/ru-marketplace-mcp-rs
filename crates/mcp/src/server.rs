use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use ru_marketplace_aliexpress::AliExpressConnector;
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
use ru_marketplace_taobao::TaobaoConnector;
use ru_marketplace_wb::{WB_DEFAULT_DEST, WildberriesConnector};
use ru_marketplace_yandex::YandexConnector;

/// MCP Stdio JSON-RPC 2.0 Server.
pub struct McpServer {
    aggregator: AggregatorService,
    wb: Arc<WildberriesConnector>,
    ozon: Arc<OzonConnector>,
    dns: Arc<DnsConnector>,
    citilink: Arc<CitilinkConnector>,
    yandex: Arc<YandexConnector>,
    aliexpress: Arc<AliExpressConnector>,
    taobao: Arc<TaobaoConnector>,
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
        let yandex = Arc::new(YandexConnector::with_solver(Some(solver.clone())));
        let aliexpress = Arc::new(AliExpressConnector::with_solver(Some(solver.clone())));
        let taobao = Arc::new(TaobaoConnector::with_solver(solver.clone()));

        let connectors: Vec<Arc<dyn MarketplaceConnector>> = vec![
            wb.clone(),
            ozon.clone(),
            yandex.clone(),
            dns.clone(),
            citilink.clone(),
            aliexpress.clone(),
            taobao.clone(),
        ];

        let aggregator = AggregatorService::new(connectors);

        Self {
            aggregator,
            wb,
            ozon,
            dns,
            citilink,
            yandex,
            aliexpress,
            taobao,
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
                description: "Price one product across Russian and international marketplaces (Wildberries, Ozon, Yandex Market, DNS, Citilink, AliExpress, Taobao) concurrently and rank by price.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Russian search text, e.g. 'iphone 15 128' or 'стиральная машина узкая'"
                        },
                        "marketplace": {
                            "type": "string",
                            "description": "Optional single marketplace filter (wildberries, ozon, yandex, dns, citilink, aliexpress, taobao)"
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
                name: "dns_card".to_string(),
                description: "Get detailed product card information for a DNS-Shop item by ID or URL slug.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "DNS product ID (e.g. '543210' or product slug from URL)"
                        }
                    },
                    "required": ["item_id"]
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
                name: "citilink_card".to_string(),
                description: "Get detailed product card information for a Citilink item by ID or URL slug.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "Citilink product ID (e.g. '1928374' or slug from URL)"
                        }
                    },
                    "required": ["item_id"]
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
            McpTool {
                name: "yandex_search".to_string(),
                description: "Search Yandex Market catalog via fast SSR JSON extraction without headless browser.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Russian search query" },
                        "limit": { "type": "integer", "description": "Maximum offers to return (default 10)", "minimum": 1, "maximum": 50 },
                        "min_price": { "type": "integer", "description": "Minimum price in rubles", "minimum": 0 },
                        "max_price": { "type": "integer", "description": "Maximum price in rubles", "minimum": 0 },
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
                name: "yandex_card".to_string(),
                description: "Get detailed product card information for a Yandex Market item by ID or URL slug.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "Yandex Market SKU, model ID or product URL path"
                        }
                    },
                    "required": ["item_id"]
                }),
            },
            McpTool {
                name: "wb_cards_batch".to_string(),
                description: "Get detailed product cards for up to 100 Wildberries items in a single batch request.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of Wildberries articles / nm IDs (up to 100)"
                        }
                    },
                    "required": ["item_ids"]
                }),
            },
            McpTool {
                name: "wb_root_info".to_string(),
                description: "Get Wildberries item root info (imt_id, category, vendor code) needed for reviews and questions.".to_string(),
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
                name: "wb_reviews".to_string(),
                description: "Get customer reviews for a Wildberries product pool by imt_id (model root ID).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "imt_id": {
                            "type": "integer",
                            "description": "Wildberries root model ID (imt_id), obtained via wb_root_info"
                        },
                        "page": {
                            "type": "integer",
                            "description": "Page number (default 1)",
                            "minimum": 1
                        }
                    },
                    "required": ["imt_id"]
                }),
            },
            McpTool {
                name: "wb_seller".to_string(),
                description: "Get Wildberries seller legal information (OGRN, INN, legal address) by supplier_id.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "supplier_id": {
                            "type": "integer",
                            "description": "Wildberries supplier/seller numeric ID"
                        }
                    },
                    "required": ["supplier_id"]
                }),
            },
            McpTool {
                name: "ozon_reviews".to_string(),
                description: "Get customer reviews, aspects, and star distribution for an Ozon item by SKU or URL.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "sku_or_url": {
                            "type": "string",
                            "description": "Ozon product SKU or product URL"
                        },
                        "page": {
                            "type": "integer",
                            "description": "Page number (default 1)",
                            "minimum": 1
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sorting criteria for reviews (e.g. 'score_desc', 'score_asc', 'useful_desc')",
                            "default": "score_desc"
                        }
                    },
                    "required": ["sku_or_url"]
                }),
            },
            McpTool {
                name: "compare_verify_offer".to_string(),
                description: "Verify an offer against live marketplace data to confirm current price and stock status.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "marketplace": {
                            "type": "string",
                            "description": "Marketplace identifier ('wildberries', 'ozon', 'yandex', 'dns', 'citilink')"
                        },
                        "item_id": {
                            "type": "string",
                            "description": "Product ID, SKU or URL"
                        },
                        "expected_price": {
                            "type": "integer",
                            "description": "Expected price in rubles to compare against current live price"
                        }
                    },
                    "required": ["marketplace", "item_id", "expected_price"]
                }),
            },
            McpTool {
                name: "compare_browser_snapshot".to_string(),
                description: "Capture a full-page JPEG screenshot of a marketplace product page via Chromium CDP for Vision models (returns base64 JPEG).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Target marketplace URL to render and capture"
                        },
                        "quality": {
                            "type": "integer",
                            "description": "JPEG quality (1-100, default 75)",
                            "minimum": 1,
                            "maximum": 100
                        }
                    },
                    "required": ["url"]
                }),
            },
            McpTool {
                name: "compare_sources".to_string(),
                description: "List all supported Russian marketplaces and their extraction capabilities.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            McpTool {
                name: "wb_categories".to_string(),
                description: "Browse the Wildberries catalog category tree with depth and subtree filtering.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "root": {
                            "type": "string",
                            "description": "'top' for the top-level sections, or a category name / URL path / id to expand (e.g. 'Электроника', '/catalog/elektronika', '8126')",
                            "default": "top"
                        },
                        "max_depth": {
                            "type": "integer",
                            "description": "How many levels below the root to include. 1 = direct children only.",
                            "minimum": 1,
                            "maximum": 3,
                            "default": 1
                        }
                    }
                }),
            },
            McpTool {
                name: "wb_category_products".to_string(),
                description: "List products in a Wildberries catalog category using shard and query selectors from wb_categories.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "shard": {
                            "type": "string",
                            "description": "WB catalog shard from wb_categories, e.g. 'electronic58'"
                        },
                        "query": {
                            "type": "string",
                            "description": "WB catalog selector from wb_categories, e.g. 'cat=9845' or 'subject=1234'"
                        },
                        "page": {
                            "type": "integer",
                            "description": "Page number (up to 100 products per page)",
                            "minimum": 1,
                            "maximum": 100,
                            "default": 1
                        },
                        "sort": {
                            "type": "string",
                            "description": "Sort order: popular, priceup, pricedown, rate, newly",
                            "enum": ["popular", "priceup", "pricedown", "rate", "newly"],
                            "default": "popular"
                        },
                        "dest": {
                            "type": "string",
                            "description": "Optional WB region ID (defaults to Moscow region)"
                        }
                    },
                    "required": ["shard", "query"]
                }),
            },
            McpTool {
                name: "aliexpress_search".to_string(),
                description: "Search AliExpress Russia catalog with authentic browser session bypass (x5sec).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "description": "Maximum offers to return (default 10)", "minimum": 1, "maximum": 50 },
                        "min_price": { "type": "integer", "description": "Minimum price in rubles", "minimum": 0 },
                        "max_price": { "type": "integer", "description": "Maximum price in rubles", "minimum": 0 },
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
                name: "aliexpress_card".to_string(),
                description: "Get detailed product card information for an AliExpress Russia item by 9-16 digit item ID or URL.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "AliExpress 9-16 digit item ID or aliexpress.ru item URL"
                        }
                    },
                    "required": ["item_id"]
                }),
            },
            McpTool {
                name: "taobao_search".to_string(),
                description: "Search Taobao marketplace catalog with prices kept in Chinese Yuan (CNY).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "description": "Maximum offers to return (default 10)", "minimum": 1, "maximum": 50 },
                        "min_price": { "type": "integer", "description": "Minimum price in yuan (CNY)", "minimum": 0 },
                        "max_price": { "type": "integer", "description": "Maximum price in yuan (CNY)", "minimum": 0 },
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
                name: "taobao_card".to_string(),
                description: "Get detailed product card information for a Taobao item by numeric item ID or item URL (prices in CNY).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "string",
                            "description": "Taobao 9-13 digit item ID or item.taobao.com URL"
                        }
                    },
                    "required": ["item_id"]
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
            "dns_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.dns.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("DNS card error: {e}")),
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
            "citilink_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.citilink.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Citilink card error: {e}")),
                }
            }
            "yandex_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.yandex.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Yandex Market search error: {e}")),
                }
            }
            "yandex_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.yandex.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Yandex Market card error: {e}")),
                }
            }
            "wb_cards_batch" => {
                let ids_val = args.get("item_ids");
                let mut nm_ids = Vec::new();
                if let Some(arr) = ids_val.and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            if let Ok(id) = s.parse::<u64>() {
                                nm_ids.push(id);
                            }
                        } else if let Some(n) = item.as_u64() {
                            nm_ids.push(n);
                        }
                    }
                }
                if nm_ids.is_empty() {
                    return McpCallResult::error(
                        "Argument 'item_ids' must contain at least one valid numeric ID",
                    );
                }
                match self.wb.get_cards_batch(&nm_ids).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries cards batch error: {e}")),
                }
            }
            "wb_root_info" => {
                let nm_id = if let Some(s) = args.get("item_id").and_then(|v| v.as_str()) {
                    s.parse::<u64>().unwrap_or(0)
                } else {
                    args.get("item_id").and_then(|v| v.as_u64()).unwrap_or(0)
                };
                if nm_id == 0 {
                    return McpCallResult::error(
                        "Argument 'item_id' must be a valid numeric Wildberries article ID",
                    );
                }
                match self.wb.get_root_info(nm_id).await {
                    Ok(info) => match serde_json::to_string_pretty(&info) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries root info error: {e}")),
                }
            }
            "wb_reviews" => {
                let imt_id = args.get("imt_id").and_then(|v| v.as_u64()).unwrap_or(0);
                let page = args
                    .get("page")
                    .and_then(|v| v.as_u64())
                    .map(|p| p as usize)
                    .unwrap_or(1);
                if imt_id == 0 {
                    return McpCallResult::error("Argument 'imt_id' must be a valid numeric ID");
                }
                match self.wb.get_reviews(imt_id, page).await {
                    Ok(reviews) => match serde_json::to_string_pretty(&reviews) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries reviews error: {e}")),
                }
            }
            "wb_seller" => {
                let supplier_id = args
                    .get("supplier_id")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if supplier_id == 0 {
                    return McpCallResult::error(
                        "Argument 'supplier_id' must be a valid numeric ID",
                    );
                }
                match self.wb.get_seller_info(supplier_id).await {
                    Ok(seller) => match serde_json::to_string_pretty(&seller) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries seller error: {e}")),
                }
            }
            "ozon_reviews" => {
                let sku_or_url = args
                    .get("sku_or_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let page = args
                    .get("page")
                    .and_then(|v| v.as_u64())
                    .map(|p| p as usize)
                    .unwrap_or(1);
                let sort = args
                    .get("sort")
                    .and_then(|v| v.as_str())
                    .unwrap_or("score_desc");
                if sku_or_url.trim().is_empty() {
                    return McpCallResult::error("Argument 'sku_or_url' must not be empty");
                }
                match self.ozon.get_reviews(sku_or_url, page, sort).await {
                    Ok(reviews) => match serde_json::to_string_pretty(&reviews) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Ozon reviews error: {e}")),
                }
            }
            "compare_verify_offer" => {
                let m_str = args
                    .get("marketplace")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let expected_price = args
                    .get("expected_price")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if m_str.is_empty() || item_id.is_empty() {
                    return McpCallResult::error(
                        "Arguments 'marketplace' and 'item_id' must not be empty",
                    );
                }
                let m = match Marketplace::from_str(m_str) {
                    Ok(m) => m,
                    Err(e) => return McpCallResult::error(format!("Invalid marketplace: {e}")),
                };
                let connector: &dyn MarketplaceConnector = match m {
                    Marketplace::Wildberries => self.wb.as_ref(),
                    Marketplace::Ozon => self.ozon.as_ref(),
                    Marketplace::YandexMarket => self.yandex.as_ref(),
                    Marketplace::Dns => self.dns.as_ref(),
                    Marketplace::Citilink => self.citilink.as_ref(),
                    Marketplace::AliExpress => self.aliexpress.as_ref(),
                    Marketplace::Taobao => self.taobao.as_ref(),
                    _ => {
                        return McpCallResult::error(format!(
                            "Unsupported marketplace for verify: {m}"
                        ));
                    }
                };
                let original_offer = ru_marketplace_core::domain::ProductOffer {
                    marketplace: m,
                    title: String::new(),
                    url: item_id.to_string(),
                    price_tier: ru_marketplace_core::domain::PriceTier::Base(expected_price),
                    original_price: None,
                    currency: if m == Marketplace::Taobao {
                        ru_marketplace_core::domain::Currency::Cny
                    } else {
                        ru_marketplace_core::domain::Currency::Rub
                    },
                    rating: None,
                    review_count: None,
                    stock: ru_marketplace_core::domain::StockStatus::InStock(1),
                    seller: None,
                    product_id: Some(item_id.to_string()),
                };
                match ru_marketplace_core::verify_offer(connector, &original_offer).await {
                    Ok(res) => match serde_json::to_string_pretty(&res) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Offer verification error: {e}")),
                }
            }
            "compare_browser_snapshot" => {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or_default();
                let quality = args
                    .get("quality")
                    .and_then(|v| v.as_u64())
                    .map(|q| q as u8)
                    .unwrap_or(75);
                if url.trim().is_empty() {
                    return McpCallResult::error("Argument 'url' must not be empty");
                }
                match self.solver.capture_screenshot(url, quality).await {
                    Ok(jpeg_bytes) => {
                        let b64 = ru_marketplace_solver::encode_base64(&jpeg_bytes);
                        let result = json!({
                            "format": "jpeg",
                            "quality": quality,
                            "size_bytes": jpeg_bytes.len(),
                            "base64_data": b64,
                            "data_uri": format!("data:image/jpeg;base64,{b64}")
                        });
                        match serde_json::to_string_pretty(&result) {
                            Ok(text) => McpCallResult::text(text),
                            Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                        }
                    }
                    Err(e) => McpCallResult::error(format!("Screenshot capture error: {e}")),
                }
            }
            "compare_sources" => {
                let sources = json!([
                    {
                        "marketplace": "wildberries",
                        "features": ["search", "card", "cards_batch", "reviews", "seller", "root_info"],
                        "latency": "~50ms (cards) / ~200ms (search)",
                        "transport": "CDN shards + Solver CDP"
                    },
                    {
                        "marketplace": "ozon",
                        "features": ["search", "card", "reviews"],
                        "latency": "~250ms",
                        "transport": "Composer API + Solver CDP fallback"
                    },
                    {
                        "marketplace": "yandex",
                        "features": ["search", "card"],
                        "latency": "~150ms",
                        "transport": "Direct SSR JSON extraction"
                    },
                    {
                        "marketplace": "dns",
                        "features": ["search", "card"],
                        "latency": "~300ms",
                        "transport": "HTTP Microdata/DOM + Solver CDP fallback"
                    },
                    {
                        "marketplace": "citilink",
                        "features": ["search", "card"],
                        "latency": "~200ms",
                        "transport": "Next.js JSON/DOM + Solver CDP fallback"
                    },
                    {
                        "marketplace": "aliexpress",
                        "features": ["search", "card"],
                        "latency": "~500ms",
                        "transport": "HTTP / NativeCdpSolver x5sec fallback"
                    },
                    {
                        "marketplace": "taobao",
                        "features": ["search", "card"],
                        "latency": "~800ms",
                        "transport": "NativeCdpSolver signed mtop API (prices in CNY)"
                    },
                ]);
                match serde_json::to_string_pretty(&sources) {
                    Ok(text) => McpCallResult::text(text),
                    Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                }
            }
            "wb_categories" => {
                let root = args.get("root").and_then(|v| v.as_str()).unwrap_or("top");
                let max_depth = args
                    .get("max_depth")
                    .and_then(|v| v.as_u64())
                    .map(|d| d as usize)
                    .unwrap_or(1);
                match self.wb.get_categories(root, max_depth).await {
                    Ok(resp) => match serde_json::to_string_pretty(&resp) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Wildberries categories error: {e}")),
                }
            }
            "wb_category_products" => {
                let shard = args
                    .get("shard")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let cat_query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let page = args
                    .get("page")
                    .and_then(|v| v.as_u64())
                    .map(|p| p as usize)
                    .unwrap_or(1);
                let sort_str = args.get("sort").and_then(|v| v.as_str());
                let dest_str = args.get("dest").and_then(|v| v.as_str());
                if shard.trim().is_empty() || cat_query.trim().is_empty() {
                    return McpCallResult::error(
                        "Arguments 'shard' and 'query' must not be empty (take them from wb_categories)",
                    );
                }
                match self
                    .wb
                    .get_category_products_full(shard, cat_query, page, sort_str, dest_str)
                    .await
                {
                    Ok(resp) => match serde_json::to_string_pretty(&resp) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => {
                        McpCallResult::error(format!("Wildberries category products error: {e}"))
                    }
                }
            }
            "aliexpress_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.aliexpress.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("AliExpress search error: {e}")),
                }
            }
            "aliexpress_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.aliexpress.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("AliExpress card error: {e}")),
                }
            }
            "taobao_search" => {
                if query.trim().is_empty() {
                    return McpCallResult::error("Argument 'query' must not be empty");
                }
                match self.taobao.search(query, limit.max(10), &filters).await {
                    Ok(offers) => match serde_json::to_string_pretty(&offers) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Taobao search error: {e}")),
                }
            }
            "taobao_card" => {
                let item_id = args
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if item_id.trim().is_empty() {
                    return McpCallResult::error("Argument 'item_id' must not be empty");
                }
                match self.taobao.get_card(item_id).await {
                    Ok(offer) => match serde_json::to_string_pretty(&offer) {
                        Ok(text) => McpCallResult::text(text),
                        Err(e) => McpCallResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpCallResult::error(format!("Taobao card error: {e}")),
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
