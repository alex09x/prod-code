/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

use crate::protocol::{
    self, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION,
};
use crate::server::failover::{is_retryable_connection_error, rediscover_node};
use crate::tools::{execute_tool, list_tools};

/// Server-safe maximum ceiling for MCP tool call execution to prevent client context deadline exceeded (180s).
pub const MCP_TOOL_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// Resolve effective tool call execution timeout from arguments.
/// Caller-supplied `timeout_secs` is clamped to `MCP_TOOL_CALL_TIMEOUT` to ensure a server-safe bound.
pub fn resolve_tool_call_timeout(arguments: &serde_json::Value) -> std::time::Duration {
    arguments
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .map(|s| std::time::Duration::from_secs(s.max(1).saturating_add(1)))
        .map(|dur| dur.min(MCP_TOOL_CALL_TIMEOUT))
        .unwrap_or(MCP_TOOL_CALL_TIMEOUT)
}

/// Process a single incoming MCP JSON-RPC message.
/// Returns Ok(Some(response)) for requests that require a response, or Ok(None) for notifications.
pub async fn handle_mcp_request(
    remote: &mut SocketAddr,
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
                        "tools": { "listChanged": true }
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    },
                    "instructions": crate::protocol::AGENT_INSTRUCTIONS
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

            let call_timeout = resolve_tool_call_timeout(&arguments);

            let tool_execution = async {
                let tool_fut = execute_tool(*remote, workspace_root, tool_name, arguments.clone());
                match tool_fut.await {
                    Ok(call_result) => Ok(call_result),
                    Err(e) if is_retryable_connection_error(tool_name, &e) => {
                        // Gateway unreachable before dispatch — try UDP discovery to find a live node.
                        tracing::warn!(
                            old_gateway = %remote,
                            error = %e,
                            tool = tool_name,
                            "gateway unreachable, running UDP discovery"
                        );
                        let old = *remote;
                        if let Some(new_addr) = rediscover_node(old, workspace_root).await {
                            *remote = new_addr;
                            let identity = crate::sync::workspace_identity(workspace_root);
                            let name = identity.base.unwrap_or(identity.name);
                            crate::cluster::remember_placement(&name, new_addr);
                            tracing::info!(
                                old = %old,
                                new = %new_addr,
                                "re-placed to a live gateway via UDP discovery"
                            );
                            // Retry the tool call on the new node.
                            execute_tool(*remote, workspace_root, tool_name, arguments).await
                        } else {
                            Err(e)
                        }
                    }
                    Err(e) => Err(e),
                }
            };

            let executed = match tokio::time::timeout(call_timeout, tool_execution).await {
                Ok(res) => res,
                Err(_) => {
                    let secs = call_timeout.as_secs();
                    tracing::warn!(
                        tool = tool_name,
                        secs,
                        "tool call exceeded MCP timeout budget"
                    );
                    let call_result = protocol::McpToolCallResult::error(format!(
                        "Tool call '{tool_name}' timed out after {secs}s\n\n\
                         💡 If this is an unexpected error or a bug in prod-code, please report it:\n\
                         - Via MCP: call `code_report_issue` with `title` and `body` (automatically sanitizes hostnames, LAN addresses, and home paths)\n\
                         - Via CLI: `prod-code report-issue --title \"...\" --body \"...\"` (automatically sanitizes hostnames, LAN addresses, and home paths)\n\
                         - On GitHub: https://github.com/alex09x/prod-code/issues (manually remove hostnames, LAN addresses, home paths, and credentials before posting)"
                    ));
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    return Ok(Some(serde_json::to_value(resp)?));
                }
            };

            match executed {
                Ok(call_result) => {
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    Ok(Some(serde_json::to_value(resp)?))
                }
                Err(e) => {
                    let err_msg = format!(
                        "{e}\n\n\
                         💡 If this is an unexpected error or a bug in prod-code, please report it:\n\
                         - Via MCP: call `code_report_issue` with `title` and `body` (automatically sanitizes hostnames, LAN addresses, and home paths)\n\
                         - Via CLI: `prod-code report-issue --title \"...\" --body \"...\"` (automatically sanitizes hostnames, LAN addresses, and home paths)\n\
                         - On GitHub: https://github.com/alex09x/prod-code/issues (manually remove hostnames, LAN addresses, home paths, and credentials before posting)"
                    );
                    let call_result = protocol::McpToolCallResult::error(err_msg);
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
