/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Native Model Context Protocol (MCP) server for prod-code AI agent fleets.

pub mod call_tree;
pub mod caller_migration;
pub mod cluster;
pub mod codemod;
pub mod compile_check;
pub mod dataflow;
pub mod dead_code;
pub mod dependencies;
pub mod diagnostics;
pub mod dossier;
pub mod duplicates;
pub mod encapsulate_field;
pub mod exec;
pub mod expression_synthesis;
pub mod extract_delegate;
pub mod extract_field;
pub mod extract_function;
pub mod extract_function_polyglot;
pub mod extract_interface;
pub mod extract_parameter;
pub mod extract_trait;
pub mod fixit;
pub mod fixture;
pub mod generify;
pub mod hot_reload;
pub mod impact;
pub mod inline_parameter;
pub mod introduce_variable;
pub mod invert_boolean;
pub mod invert_value;
pub mod lang;
pub mod loop_to_iterator;
pub mod make_static;
pub mod markdown;
pub mod move_item;
pub mod move_method;
pub mod move_module;
pub mod move_polyglot;
pub mod parameter_object;
pub mod patch;
pub mod protocol;
pub mod prune;
pub mod pull_push;
pub mod reachability;
pub mod refactor;
pub mod remote_fs;
pub mod rename_accessors;
pub mod rename_mentions;
pub mod replace_conditional;
pub mod replace_constructor;
pub mod replace_inheritance;
pub mod report;
pub mod safe_delete_go;
pub mod safe_delete_typescript;
pub mod schema;
pub mod search;
pub mod session;
pub mod shadow;
pub mod signature;
pub mod signature_go;
pub mod signature_polyglot;
pub mod slice;
pub mod supertypes;
pub mod sync;
pub mod to_method;
pub mod tools;
pub mod trait_param;
pub mod type_migration;
pub mod verify;
pub mod watch;
pub mod wrap_return;
pub mod xml_svg;

pub use protocol::{MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};
pub use sync::scan_workspace_files;
pub use tools::{execute_tool, list_tools};

use anyhow::{Context, Result};
use futures_util::FutureExt;
use protocol::{JsonRpcRequest, JsonRpcResponse};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
                    tracing::warn!(tool = tool_name, secs, "tool call exceeded MCP timeout budget");
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

/// Run full stdio MCP server loop.
pub async fn run_stdio_mcp_server(remote: SocketAddr, workspace_root: PathBuf) -> Result<()> {
    let resumed = std::env::var_os(hot_reload::RESUMED_ENV).is_some();
    tracing::info!(
        server = SERVER_NAME,
        version = SERVER_VERSION,
        gateway = %remote,
        workspace = %workspace_root.display(),
        resumed,
        "Starting prod-code stdio MCP server"
    );

    // Hot reload: watch our own executable and swap to a newly installed one between
    // requests (see `hot_reload`).
    let exe = std::env::current_exe().ok();
    serve_mcp_requests(
        remote,
        workspace_root,
        exe,
        resumed,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}

/// Whether an error is an initial transport connection failure that can be safely retried.
///
/// Mutating tools (`code_exec`) must NEVER be automatically retried: if the gateway received
/// the command and dropped connection during execution, replaying it on another node can
/// cause duplicate external side-effects (e.g. database migrations, external API calls).
/// Similarly, any error where the command result is unknown is non-retryable.
pub fn is_retryable_connection_error(tool_name: &str, e: &anyhow::Error) -> bool {
    if tool_name == "code_exec" {
        return false;
    }
    let msg = format!("{e:#}").to_lowercase();
    if msg.contains("result is unknown")
        || msg.contains("during exec")
        || msg.contains("command's result is unknown")
    {
        return false;
    }
    msg.contains("failed to connect to remote gateway")
        || msg.contains("connection refused")
        || msg.contains("no route to host")
        || msg.contains("network is unreachable")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("broken pipe")
        || msg.contains("connection reset")
        || msg.contains("connection closed")
        || msg.contains("transport closed")
        || msg.contains("server closed connection")
        || msg.contains("capacity: this node has no memory")
        || msg.contains("gateway refused the session: capacity")
        || msg.contains("this node has no memory for a new")
        || msg.contains("refused for capacity")
        || msg.contains("capacity admission")
        || msg.contains("no space left on device")
        || msg.contains("short of disk")
}

/// Run a 250ms UDP discovery probe (multicast + unicast to the old address) and return
/// the best live node that can serve this workspace's engine and OS requirements.
///
/// Routing priority:
/// 1. Only consider nodes that support the required engine and OS (e.g., Swift requires macOS)
/// 2. Nodes that already have the current workspace loaded (warm engine, no cold start)
/// 3. Among those (or all candidates if none has it), pick the one with the most available memory
///    and the lowest load.
pub async fn rediscover_node(old: SocketAddr, workspace_root: &std::path::Path) -> Option<SocketAddr> {
    let seeds = vec![old];
    let nodes = tokio::task::spawn_blocking(move || {
        prod_code_protocol::discovery::discover(&seeds)
    })
    .await
    .ok()?;

    if nodes.is_empty() {
        return None;
    }

    // Detect required engine and OS for this workspace.
    let (subproject, detected_engine) = crate::sync::engine_project(workspace_root, workspace_root);
    let macos_cgo = match detected_engine {
        Some("go") => {
            let target_dir = subproject
                .as_ref()
                .map(|sub| workspace_root.join(sub))
                .unwrap_or_else(|| workspace_root.to_path_buf());
            crate::sync::macos_only_cgo(&target_dir)
        }
        _ => None,
    };
    let needs_macos = detected_engine == Some("swift") || macos_cgo.is_some();

    // Log what we found.
    for n in &nodes {
        tracing::debug!(
            addr = %n.addr,
            engines = ?n.engines,
            cpus = n.cpus,
            mem_avail_mb = n.mem_avail_mb,
            mem_total_mb = n.mem_total_mb,
            rss_mb = n.rss_mb,
            load = n.load_per_cpu,
            sessions = n.sessions,
            workspaces = n.workspaces.len(),
            "discovered node"
        );
    }

    // Exclude the dead node (unless it's the only one that answered).
    let viable: Vec<_> = nodes.iter().filter(|n| n.addr != old).collect();
    let viable = if viable.is_empty() { nodes.iter().collect() } else { viable };

    // Filter by required engine and OS!
    let candidates: Vec<_> = viable
        .into_iter()
        .filter(|n| {
            if let Some(engine) = detected_engine {
                let has_engine = n.engines.iter().any(|e| {
                    e.as_str() == engine
                        || e.strip_prefix(engine).is_some_and(|rest| rest.starts_with(' '))
                });
                if !has_engine {
                    return false;
                }
            }
            if needs_macos {
                let has_macos = n.engines.iter().any(|e| e == "swift" || e.starts_with("swift "));
                if !has_macos {
                    return false;
                }
            }
            true
        })
        .collect();

    if candidates.is_empty() {
        tracing::warn!(
            engine = ?detected_engine,
            needs_macos,
            "no discovered nodes support the required engine/OS for this workspace"
        );
        return None;
    }

    // The workspace identity (directory basename) — matches what the gateway uses.
    let ws_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Prefer a node that already has this workspace loaded (warm engine),
    // provided it has sufficient memory (at least 512 MB available).
    let warm: Vec<_> = candidates
        .iter()
        .filter(|n| n.workspaces.iter().any(|w| w.name == ws_name) && n.mem_avail_mb >= 512)
        .copied()
        .collect();

    // Prefer nodes with at least 512 MB available memory if any exist.
    let roomy: Vec<_> = candidates
        .iter()
        .filter(|n| n.mem_avail_mb >= 512)
        .copied()
        .collect();

    let pool = if !warm.is_empty() {
        &warm
    } else if !roomy.is_empty() {
        &roomy
    } else {
        &candidates
    };

    // Score: more available memory is better, lower load is better.
    pool.iter()
        .max_by(|a, b| {
            let score_a = a.mem_avail_mb as f64 - a.load_per_cpu * 10000.0;
            let score_b = b.mem_avail_mb as f64 - b.load_per_cpu * 10000.0;
            score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|n| n.addr)
}

/// The request/response loop, generic over its transport so it can be driven by a test without
/// touching the process's real stdio. `run_stdio_mcp_server` is a thin wrapper around this with
/// the real standard streams; `exe` is `None` there only when the running binary's own path
/// could not be resolved, in which case hot reload is simply not offered.
async fn serve_mcp_requests<R, W>(
    mut remote: SocketAddr,
    workspace_root: PathBuf,
    exe: Option<PathBuf>,
    resumed: bool,
    mut reader: BufReader<R>,
    mut writer: W,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let reload_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reload_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    if let Some(exe) = &exe
        && let Some(initial) = hot_reload::stamp(exe)
    {
        hot_reload::spawn_watch(
            exe.clone(),
            initial,
            std::sync::Arc::clone(&reload_flag),
            std::sync::Arc::clone(&reload_notify),
        );
    }
    if resumed {
        // The session was initialised with the previous binary: tell the client to refresh
        // its tool list from this one.
        writer
            .write_all(hot_reload::tools_list_changed().as_bytes())
            .await?;
        writer.flush().await?;
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut rebalance_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    rebalance_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if reload_flag.load(std::sync::atomic::Ordering::Acquire)
            && pending.is_empty()
            && reader.buffer().is_empty()
            && let Some(exe) = &exe
        {
            writer
                .write_all(hot_reload::tools_list_changed().as_bytes())
                .await?;
            writer.flush().await?;
            tracing::info!(exe = %exe.display(), "re-executing the installed binary");
            let err = hot_reload::reexec(exe);
            tracing::error!(error = %err, "hot reload failed; continuing with the current binary");
            reload_flag.store(false, std::sync::atomic::Ordering::Release);
        }

        let consumed = tokio::select! {
            filled = reader.fill_buf() => {
                let buf = filled.context("Failed reading from stdin")?;
                if buf.is_empty() {
                    break; // EOF
                }
                pending.extend_from_slice(buf);
                buf.len()
            }
            _ = reload_notify.notified() => 0,
            _ = rebalance_interval.tick() => {
                let identity = crate::sync::workspace_identity(&workspace_root);
                let ws_name = identity.base.as_ref().unwrap_or(&identity.name).clone();
                let (_, engine) = crate::sync::engine_project(&workspace_root, &workspace_root);
                let os = crate::sync::macos_only_cgo(&workspace_root).map(|_| "macos");
                if let Some((new_addr, reason)) = crate::cluster::evaluate_cluster_rebalance(
                    remote,
                    &ws_name,
                    engine,
                    os,
                ).await {
                    tracing::info!(
                        old = %remote,
                        new = %new_addr,
                        %reason,
                        "cluster rebalance: migrating active workspace to more efficient node"
                    );
                    remote = new_addr;
                    crate::cluster::remember_placement(&ws_name, new_addr);
                    // Pre-warm the workspace on the new node in the background
                    let root_clone = workspace_root.clone();
                    let identity_clone = identity.clone();
                    tokio::spawn(async move {
                        if let Ok(stream) = prod_code_protocol::transport::connect(new_addr).await {
                            let mut framed = tokio_util::codec::Framed::new(
                                stream,
                                prod_code_protocol::ProdCodeCodec::new(),
                            );
                            let _ = crate::sync::push_workspace_sync(
                                &mut framed,
                                &root_clone,
                                &identity_clone,
                                None,
                            )
                            .await;
                        }
                    });
                }
                0
            }
        };
        reader.consume(consumed);

        while let Some(line) = hot_reload::take_line(&mut pending) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req_json = match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    let err_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": { "code": -32700, "message": format!("Parse error: {e}") }
                    });
                    if let Ok(mut out) = serde_json::to_string(&err_resp) {
                        out.push('\n');
                        let _ = writer.write_all(out.as_bytes()).await;
                        let _ = writer.flush().await;
                    }
                    continue;
                }
            };
            let req_id = req_json.get("id").cloned();
            let handle_future = std::panic::AssertUnwindSafe(
                handle_mcp_request(&mut remote, &workspace_root, req_json)
            );
            match handle_future.catch_unwind().await {
                Ok(Ok(Some(resp_val))) => {
                    let out = match serde_json::to_string(&resp_val) {
                        Ok(mut s) => {
                            s.push('\n');
                            s
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to serialize MCP response");
                            serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": { "code": -32603, "message": format!("Serialization error: {e}") }
                            })
                            .to_string()
                            + "\n"
                        }
                    };
                    if let Err(e) = writer.write_all(out.as_bytes()).await {
                        tracing::error!(error = %e, "Failed writing to stdout");
                        break;
                    }
                    if let Err(e) = writer.flush().await {
                        tracing::error!(error = %e, "Failed flushing stdout");
                        break;
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "MCP request handler internal error");
                    let err_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": { "code": -32603, "message": format!("Internal error: {e}") }
                    });
                    if let Ok(mut out) = serde_json::to_string(&err_resp) {
                        out.push('\n');
                        let _ = writer.write_all(out.as_bytes()).await;
                        let _ = writer.flush().await;
                    }
                }
                Err(panic_payload) => {
                    let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::error!(panic = %msg, "MCP request handler panicked");
                    let err_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": { "code": -32603, "message": format!("Internal error (panic): {msg}") }
                    });
                    if let Ok(mut out) = serde_json::to_string(&err_resp) {
                        out.push('\n');
                        let _ = writer.write_all(out.as_bytes()).await;
                        let _ = writer.flush().await;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_mcp_initialize() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
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

        let resp_opt = handle_mcp_request(&mut dummy_addr, &root, req).await.unwrap();
        assert!(resp_opt.is_some());
        let resp = resp_opt.unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["serverInfo"]["name"], "prod-code-mcp");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        // Every agent reads these, skill or not: they carry the bug-report rule too (#302).
        let instructions = resp["result"]["instructions"].as_str().unwrap_or_default();
        for needed in [
            "code_report_issue",
            "private_ref",
            "never put private details",
            "labels",
            "code_change_signature",
            "code_replace_constructor_with_builder",
            "code_shadow_run",
            "code_validate_edits",
            "code_prune_orphans",
        ] {
            assert!(
                instructions.contains(needed),
                "{needed} missing: {instructions}"
            );
        }
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        let resp_opt = handle_mcp_request(&mut dummy_addr, &root, req).await.unwrap();
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
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "ping"
        });

        let resp = handle_mcp_request(&mut dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 3);
        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_mcp_unknown_method() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "foo/bar"
        });

        let resp = handle_mcp_request(&mut dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 4);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_mcp_unknown_notification_gets_no_response() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({ "jsonrpc": "2.0", "method": "foo/bar" });

        let resp = handle_mcp_request(&mut dummy_addr, &root, req).await.unwrap();
        assert!(resp.is_none(), "a notification gets no reply: {resp:?}");
    }

    #[tokio::test]
    async fn test_mcp_initialized_notification_gets_no_response() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        for method in ["notifications/initialized", "initialized"] {
            let req = serde_json::json!({ "jsonrpc": "2.0", "method": method });
            let resp = handle_mcp_request(&mut dummy_addr, &root, req).await.unwrap();
            assert!(resp.is_none(), "{method}: {resp:?}");
        }
    }

    #[tokio::test]
    async fn test_mcp_malformed_request_is_a_parse_error() {
        let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        // Missing the required `method` field: the request itself does not deserialize.
        let resp = handle_mcp_request(&mut dummy_addr, &root, serde_json::json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["error"]["code"], -32700);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("Parse error:"),
            "{resp:?}"
        );
    }

    #[tokio::test]
    async fn test_mcp_tools_call_reports_a_failed_tool_as_a_normal_response() {
        // A closed port: the tool's own connection attempt fails, and that failure is
        // reported as a normal (non-transport) JSON-RPC response, not a handler error. Port 1,
        // not a port bound and dropped: with the whole suite running in parallel, a freed
        // ephemeral port is taken by another test's listener often enough that the connection
        // succeeds and is reset instead of refused.
        let mut dummy_addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "code_exec", "arguments": { "argv": ["true"] } }
        });

        let resp = handle_mcp_request(&mut dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 5);
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.to_lowercase().contains("failed to connect"), "{text}");
    }

    /// Wires `serve_mcp_requests` to an in-memory duplex stream: the returned handle is the
    /// client's end (write requests into it, read responses back out of it), and the join
    /// handle resolves once the client end (or its clone) is dropped, which the server reads
    /// as EOF.
    fn spawn_server(
        resumed: bool,
    ) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<Result<()>>) {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server);
        let handle = tokio::spawn(serve_mcp_requests(
            dummy_addr,
            root,
            None, // no real executable to hot-reload from in a test
            resumed,
            BufReader::new(server_read),
            server_write,
        ));
        (client, handle)
    }

    #[tokio::test]
    async fn serve_mcp_requests_answers_a_request_then_stops_at_eof() {
        let (mut client, handle) = spawn_server(false);
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();

        let mut buf = [0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        let resp: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"], serde_json::json!({}));

        drop(client); // EOF: the server's read half sees a closed connection
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn serve_mcp_requests_skips_blank_lines_and_invalid_json() {
        let (mut client, handle) = spawn_server(false);
        client
            .write_all(
                b"\n   \nnot json at all\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
            )
            .await
            .unwrap();

        // Read all output: blank lines are skipped, invalid JSON produces a parse error
        // response (-32700 with no id), and the valid ping produces a normal response.
        let mut all = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = client.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&all);
            // Stop once we've seen the ping response (id:2).
            if text.contains("\"id\":2") || text.contains("\"id\": 2") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&all);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        // Two response lines: parse error + ping.
        assert!(lines.len() >= 2, "expected ≥2 responses: {text:?}");
        let parse_err: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parse_err["error"]["code"], -32700);
        let ping: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
        assert_eq!(ping["id"], 2);
        assert_eq!(ping["result"], serde_json::json!({}));

        drop(client);
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn serve_mcp_requests_announces_a_resumed_session_before_any_request() {
        let (mut client, handle) = spawn_server(true);

        let mut buf = [0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        assert_eq!(line, hot_reload::tools_list_changed());

        drop(client);
        assert!(handle.await.unwrap().is_ok());
    }

    #[test]
    fn test_is_retryable_connection_error_rules() {
        let connect_err = anyhow::anyhow!("Failed to connect to remote gateway: Connection refused (os error 111)");
        let exec_unknown_err = anyhow::anyhow!("lost the connection to the gateway during exec (connection reset); the command's result is unknown");
        let generic_err = anyhow::anyhow!("syntax error in file.rs");
        let capacity_err = anyhow::anyhow!("the gateway refused the session: capacity: this node has no memory for a new rust engine (memory 80% used)");
        let transport_closed_err = anyhow::anyhow!("tool call failed for prod-code/code_definition: Transport closed");
        let broken_pipe_err = anyhow::anyhow!("broken pipe");

        // 1. code_exec must NEVER be retried even on connection refused or capacity
        assert!(!is_retryable_connection_error("code_exec", &connect_err));
        assert!(!is_retryable_connection_error("code_exec", &capacity_err));

        // 2. Unknown outcome during/after exec must NEVER be retried
        assert!(!is_retryable_connection_error("code_check", &exec_unknown_err));
        assert!(!is_retryable_connection_error("code_definition", &exec_unknown_err));

        // 3. Pre-dispatch connection errors for read-only tools ARE retryable
        assert!(is_retryable_connection_error("code_definition", &connect_err));
        assert!(is_retryable_connection_error("code_references", &connect_err));
        assert!(is_retryable_connection_error("code_symbols", &capacity_err));
        assert!(is_retryable_connection_error("code_definition", &transport_closed_err));
        assert!(is_retryable_connection_error("code_definition", &broken_pipe_err));

        // 4. Non-connection errors are not retryable
        assert!(!is_retryable_connection_error("code_definition", &generic_err));
    }

    #[tokio::test]
    async fn serve_mcp_requests_recovers_from_panic_and_returns_jsonrpc_error() {
        let (client, handle) = spawn_server(false);
        let mut reader = tokio::io::BufReader::new(client);

        // Ping works normally
        reader
            .get_mut()
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"], serde_json::json!({}));

        // Send a tool call with unknown tool, server handles it cleanly without dying
        line.clear();
        reader
            .get_mut()
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"non_existent_tool_12345\"}}\n")
            .await
            .unwrap();
        reader.read_line(&mut line).await.unwrap();
        let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 2);

        // Server is still alive: ping succeeds
        line.clear();
        reader
            .get_mut()
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        reader.read_line(&mut line).await.unwrap();
        let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(resp["id"], 3);

        drop(reader);
        assert!(handle.await.unwrap().is_ok());
    }
}
