use ru_marketplace_core::domain::JsonRpcRequest;
use ru_marketplace_mcp::McpServer;
use serde_json::json;

#[tokio::test]
async fn test_mcp_initialize() {
    let server = McpServer::new();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "initialize".to_string(),
        params: Some(json!({})),
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    assert_eq!(resp.id, Some(json!(1)));

    let result = resp.result.expect("Expected result");
    assert_eq!(result.get("protocolVersion").unwrap(), "2024-11-05");
    assert_eq!(
        result.get("serverInfo").unwrap().get("name").unwrap(),
        "ru-marketplace-mcp"
    );
}

#[tokio::test]
async fn test_mcp_ping() {
    let server = McpServer::new();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!("ping-1")),
        method: "ping".to_string(),
        params: None,
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    assert_eq!(resp.id, Some(json!("ping-1")));
    assert_eq!(resp.result, Some(json!({})));
}

#[tokio::test]
async fn test_mcp_tools_list() {
    let server = McpServer::new();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/list".to_string(),
        params: None,
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    let result = resp.result.expect("Expected result");
    let tools = result.get("tools").unwrap().as_array().unwrap();

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert_eq!(tools.len(), 10);
    assert!(tool_names.contains(&"compare_prices"));
    assert!(tool_names.contains(&"wb_search"));
    assert!(tool_names.contains(&"ozon_search"));
    assert!(tool_names.contains(&"dns_search"));
    assert!(tool_names.contains(&"citilink_search"));
    assert!(tool_names.contains(&"ozon_card"));
    assert!(tool_names.contains(&"wb_card"));
    assert!(tool_names.contains(&"session_start"));
    assert!(tool_names.contains(&"session_stop"));
    assert!(tool_names.contains(&"session_status"));
}

#[tokio::test]
async fn test_mcp_tools_call_validation() {
    let server = McpServer::new();

    // Call with empty query should return error
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "compare_prices",
            "arguments": {
                "query": "   "
            }
        })),
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    let result = resp.result.expect("Expected result");
    assert_eq!(result.get("isError"), Some(&json!(true)));

    // Unknown tool
    let req2 = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "non_existent_tool",
            "arguments": {
                "query": "test"
            }
        })),
    };

    let resp2 = server
        .handle_request(req2)
        .await
        .expect("Expected response");
    let result2 = resp2.result.expect("Expected result");
    assert_eq!(result2.get("isError"), Some(&json!(true)));
}

#[tokio::test]
async fn test_mcp_unknown_method() {
    let server = McpServer::new();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(5)),
        method: "some/unknown_method".to_string(),
        params: None,
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    let err = resp.error.expect("Expected error");
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn test_mcp_tools_list_schema_filters() {
    let server = McpServer::new();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(6)),
        method: "tools/list".to_string(),
        params: None,
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    let result = resp.result.expect("Expected result");
    let tools = result.get("tools").unwrap().as_array().unwrap();

    let compare_tool = tools
        .iter()
        .find(|t| t.get("name").unwrap().as_str().unwrap() == "compare_prices")
        .expect("compare_prices must exist");

    let schema = compare_tool.get("inputSchema").unwrap();
    let props = schema.get("properties").unwrap();

    assert!(props.get("min_price").is_some());
    assert!(props.get("max_price").is_some());
    assert!(props.get("sort").is_some());

    let sort_enum = props
        .get("sort")
        .unwrap()
        .get("enum")
        .unwrap()
        .as_array()
        .unwrap();

    assert!(sort_enum.contains(&json!("price_asc")));
    assert!(sort_enum.contains(&json!("reviews")));
}

#[tokio::test]
async fn test_mcp_session_tools() {
    let server = McpServer::new();

    // 1. session_status tool
    let req_status = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(90)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "session_status",
            "arguments": {
                "marketplace": "all"
            }
        })),
    };
    let resp_status = server
        .handle_request(req_status)
        .await
        .expect("Expected response");
    let result_status = resp_status.result.expect("Expected result");
    let content_status = result_status.get("content").unwrap().as_array().unwrap();
    let text_status = content_status[0].get("text").unwrap().as_str().unwrap();
    assert!(text_status.contains("ozon"));
    assert!(text_status.contains("wildberries"));

    // 2. session_stop tool
    let req_stop = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(91)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "session_stop",
            "arguments": {
                "marketplace": "all"
            }
        })),
    };
    let resp_stop = server
        .handle_request(req_stop)
        .await
        .expect("Expected response");
    let result_stop = resp_stop.result.expect("Expected result");
    let content_stop = result_stop.get("content").unwrap().as_array().unwrap();
    let text_stop = content_stop[0].get("text").unwrap().as_str().unwrap();
    assert!(text_stop.contains("No active") || text_stop.contains("stopped"));
}

#[tokio::test]
async fn test_mcp_legacy_ozon_session_aliases() {
    let server = McpServer::new();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(99)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "ozon_session_stop",
            "arguments": {}
        })),
    };
    let resp = server.handle_request(req).await.expect("Expected response");
    let result = resp.result.expect("Expected result");
    let content = result.get("content").unwrap().as_array().unwrap();
    let text = content[0].get("text").unwrap().as_str().unwrap();
    assert!(text.contains("No active browser session") || text.contains("stopped"));
}
