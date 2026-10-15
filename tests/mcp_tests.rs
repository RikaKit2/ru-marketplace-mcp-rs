use ru_marketplace_mcp::domain::JsonRpcRequest;
use ru_marketplace_mcp::mcp::McpServer;
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

    assert_eq!(tools.len(), 5);
    assert!(tool_names.contains(&"compare_prices"));
    assert!(tool_names.contains(&"wb_search"));
    assert!(tool_names.contains(&"ozon_search"));
    assert!(tool_names.contains(&"dns_search"));
    assert!(tool_names.contains(&"citilink_search"));
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
        method: "some/unknown/method".to_string(),
        params: None,
    };

    let resp = server.handle_request(req).await.expect("Expected response");
    let err = resp.error.expect("Expected error");
    assert_eq!(err.code, -32601);
}
