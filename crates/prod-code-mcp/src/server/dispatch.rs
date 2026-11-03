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
use std::path::{Path, PathBuf};

use crate::protocol::{
    self, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION,
};
use crate::server::failover::rediscover_node;
use crate::server::failover_pin::{relocation_note, retry_permitted};
use crate::tools::{execute_tool, list_tools};

/// Server-safe default timeout for MCP tool call execution when `timeout_secs` is omitted.
pub const MCP_TOOL_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// Upper ceiling for MCP tool call execution to prevent unbounded resource hold (24 hours).
pub const MAX_TOOL_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Resolve effective tool call execution timeout from arguments.
/// When caller provides `timeout_secs` (> 0), that timeout governs execution with a small margin,
/// clamped to `MAX_TOOL_CALL_TIMEOUT` (24h).
/// Otherwise falls back to `MCP_TOOL_CALL_TIMEOUT` (150s).
pub fn resolve_tool_call_timeout(arguments: &serde_json::Value) -> std::time::Duration {
    if let Some(ms) = arguments
        .get("timeout_millis")
        .and_then(|v| v.as_u64())
        .filter(|&m| m > 0)
    {
        return std::time::Duration::from_millis(ms).min(MAX_TOOL_CALL_TIMEOUT);
    }
    arguments
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .filter(|&s| s > 0)
        .map(|s| std::time::Duration::from_secs(s.saturating_add(5)).min(MAX_TOOL_CALL_TIMEOUT))
        .unwrap_or(MCP_TOOL_CALL_TIMEOUT)
}

/// Process a single incoming MCP JSON-RPC message.
/// Returns Ok(Some(response)) for requests that require a response, or Ok(None) for notifications.
pub async fn handle_mcp_request(
    remote: &mut SocketAddr,
    workspace_root: &Path,
    req_val: serde_json::Value,
) -> Result<Option<serde_json::Value>> {
    handle_mcp_request_with(remote, workspace_root, req_val, |old, root| async move {
        rediscover_node(old, &root).await
    })
    .await
}

/// [`handle_mcp_request`] with the search for a live node, after the gateway could not be
/// reached, given: `rediscover(old gateway, workspace root)`.
pub(crate) async fn handle_mcp_request_with<D, F>(
    remote: &mut SocketAddr,
    workspace_root: &Path,
    req_val: serde_json::Value,
    rediscover: D,
) -> Result<Option<serde_json::Value>>
where
    D: Fn(SocketAddr, PathBuf) -> F,
    F: std::future::Future<Output = Option<SocketAddr>>,
{
    let policy = crate::cluster::node_policy();
    handle_mcp_request_policy(remote, workspace_root, req_val, &policy, rediscover).await
}

/// [`handle_mcp_request_with`] under `policy` (#1793): a pinned workspace retries only a call
/// that never started, only on a node the pin allows, and a retried call says where it ran.
pub(crate) async fn handle_mcp_request_policy<D, F>(
    remote: &mut SocketAddr,
    workspace_root: &Path,
    req_val: serde_json::Value,
    policy: &crate::cluster::NodePolicy,
    rediscover: D,
) -> Result<Option<serde_json::Value>>
where
    D: Fn(SocketAddr, PathBuf) -> F,
    F: std::future::Future<Output = Option<SocketAddr>>,
{
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
                let mut arguments = arguments;
                let selector = if tool_name == "code_workflow" {
                    "code_definition"
                } else {
                    tool_name
                };
                let selected =
                    crate::tools::resolve_workspace_root(workspace_root, selector, &mut arguments)?;
                let selected_policy =
                    crate::cluster::pin_scope::effective_policy(&selected, policy)?;
                let home = std::fs::canonicalize(workspace_root)
                    .unwrap_or_else(|_| workspace_root.to_path_buf());
                let mut call_remote =
                    crate::cluster::pin_scope::place_within(&selected_policy, *remote)?;
                let outcome =
                    crate::cluster::with_root_policy(&selected, selected_policy.clone(), async {
                        let policy = &selected_policy;
                        match execute_tool(call_remote, &selected, tool_name, arguments.clone())
                            .await
                        {
                            Ok(call_result) => Ok(call_result),
                            Err(e)
                                if retry_permitted(policy, tool_name, &e)
                                    && (!policy.is_pinned()
                                        || !policy.alternatives(call_remote).is_empty()) =>
                            {
                                // Gateway unreachable before dispatch — try UDP discovery to find a live node.
                                tracing::warn!(
                                    old_gateway = %call_remote,
                                    error = %e,
                                    tool = tool_name,
                                    "gateway unreachable, running UDP discovery"
                                );
                                let old = call_remote;
                                if let Some(new_addr) = rediscover(old, selected.clone())
                                    .await
                                    .filter(|addr| policy.allows(*addr))
                                {
                                    call_remote = new_addr;
                                    if selected == home && !policy.is_pinned() {
                                        let identity = crate::sync::workspace_identity(&selected);
                                        let name = identity.base.unwrap_or(identity.name);
                                        crate::cluster::remember_placement(&name, new_addr);
                                    }
                                    tracing::info!(
                                        old = %old,
                                        new = %new_addr,
                                        "re-placed to a live gateway via UDP discovery"
                                    );
                                    // Retry the tool call on the new node, and say that it moved.
                                    let note = relocation_note(old, new_addr, &e);
                                    execute_tool(call_remote, &selected, tool_name, arguments)
                                        .await
                                        .map(|mut result| {
                                            result.content.push(protocol::McpContentItem::Text {
                                                text: note,
                                            });
                                            result
                                        })
                                } else {
                                    Err(e)
                                }
                            }
                            Err(e) => Err(e),
                        }
                    })
                    .await;
                // A call on a sibling checkout does not relocate the configured checkout.
                if selected == home {
                    *remote = call_remote;
                }
                outcome
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
