//! Native Model Context Protocol (MCP) server for prod-code AI agent fleets.

pub mod protocol;
pub mod sync;
pub mod tools;

pub use protocol::{MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};
pub use sync::scan_workspace_files;
pub use tools::{execute_tool, list_tools};

use anyhow::{Context, Result};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Process a single incoming MCP JSON-RPC message.
/// Returns Ok(Some(response)) for requests that require a response, or Ok(None) for notifications.
pub async fn handle_mcp_request(
    remote: SocketAddr,
    workspace_root: &Path,
    req_val: serde_json::Value,
) -> Result<Option<serde_json::Value>> {
    let req: JsonRpcRequest = match serde_json::from_value(req_val) {
        Ok(r) => r,
        Err(e) => {
            let err_resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
            return Ok(Some(serde_json::to_value(err_resp)?));
        }
    };

    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => {
            let resp = JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                }),
            );
            Ok(Some(serde_json::to_value(resp)?))
        }

        "notifications/initialized" | "initialized" => {
            // Notification: no response required
            Ok(None)
        }

        "ping" => {
            let resp = JsonRpcResponse::success(id, serde_json::json!({}));
            Ok(Some(serde_json::to_value(resp)?))
        }

        "tools/list" => {
            let tools = list_tools();
            let resp = JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "tools": tools
                }),
            );
            Ok(Some(serde_json::to_value(resp)?))
        }

        "tools/call" => {
            let params = req.params.unwrap_or(serde_json::json!({}));
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            match execute_tool(remote, workspace_root, tool_name, arguments).await {
                Ok(call_result) => {
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    Ok(Some(serde_json::to_value(resp)?))
                }
                Err(e) => {
                    let call_result = protocol::McpToolCallResult::error(e.to_string());
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    Ok(Some(serde_json::to_value(resp)?))
                }
            }
        }

        unknown => {
            // If it's a notification without an ID, do not return an error
            if id.is_none() {
                Ok(None)
            } else {
                let resp =
                    JsonRpcResponse::error(id, -32601, format!("Method not found: {unknown}"));
                Ok(Some(serde_json::to_value(resp)?))
            }
        }
    }
}

/// Run full stdio MCP server loop.
pub async fn run_stdio_mcp_server(remote: SocketAddr, workspace_root: PathBuf) -> Result<()> {
    tracing::info!(
        server = SERVER_NAME,
        version = SERVER_VERSION,
        gateway = %remote,
        workspace = %workspace_root.display(),
        "Starting prod-code stdio MCP server"
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();

    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .await
            .context("Failed reading from stdin")?;

        if bytes_read == 0 {
            // EOF reached
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(req_json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            match handle_mcp_request(remote, &workspace_root, req_json).await {
                Ok(Some(resp_val)) => {
                    let mut out = serde_json::to_string(&resp_val)?;
                    out.push('\n');
                    stdout.write_all(out.as_bytes()).await?;
                    stdout.flush().await?;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "MCP request handler internal error");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_initialize() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {}
            }
        });

        let resp_opt = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
        assert!(resp_opt.is_some());
        let resp = resp_opt.unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["serverInfo"]["name"], "prod-code-mcp");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        let resp_opt = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
        assert!(resp_opt.is_some());
        let resp = resp_opt.unwrap();
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().unwrap();
        let tool_names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

        assert!(tool_names.contains(&"code_definition"));
        assert!(tool_names.contains(&"code_references"));
        assert!(tool_names.contains(&"code_outline"));
        assert!(tool_names.contains(&"code_hover"));
        assert!(tool_names.contains(&"code_status"));
        assert!(tool_names.contains(&"code_sync"));
    }

    #[tokio::test]
    async fn test_mcp_ping() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "ping"
        });

        let resp = handle_mcp_request(dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 3);
        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_mcp_unknown_method() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "foo/bar"
        });

        let resp = handle_mcp_request(dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 4);
        assert_eq!(resp["error"]["code"], -32601);
    }
}
