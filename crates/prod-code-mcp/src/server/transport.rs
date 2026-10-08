/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::FutureExt;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::hot_reload;
use crate::protocol::{SERVER_NAME, SERVER_VERSION};
use crate::server::dispatch::handle_mcp_request;

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

/// The request/response loop, generic over its transport so it can be driven by a test without
/// touching the process's real stdio. `run_stdio_mcp_server` is a thin wrapper around this with
/// the real standard streams; `exe` is `None` there only when the running binary's own path
/// could not be resolved, in which case hot reload is simply not offered.
pub async fn serve_mcp_requests<R, W>(
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
                crate::server::rebalance::handle_cluster_rebalance_tick(
                    &mut remote,
                    &workspace_root,
                )
                .await;
                0
            }
        };
        reader.consume(consumed);

        while let Some(line) = hot_reload::take_line(&mut pending) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.len() > MAX_JSONRPC_FRAME_BYTES {
                let err_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": serde_json::Value::Null,
                    "error": {
                        "code": -32600,
                        "message": "Request exceeds maximum JSON-RPC frame size"
                    }
                });
                if let Ok(mut out) = serde_json::to_string(&err_resp) {
                    out.push('\n');
                    let _ = writer.write_all(out.as_bytes()).await;
                    let _ = writer.flush().await;
                }
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
            let handle_future = std::panic::AssertUnwindSafe(handle_mcp_request(
                &mut remote,
                &workspace_root,
                req_json,
            ));
            match handle_future.catch_unwind().await {
                Ok(Ok(Some(resp_val))) => {
                    let mut out = bound_serialized_response(resp_val, MAX_JSONRPC_FRAME_BYTES);
                    out.push('\n');
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

pub const MAX_JSONRPC_FRAME_BYTES: usize = 512 * 1024;

/// Serializes an MCP response value into a single JSON line bounded by `max_frame_bytes` to
/// avoid crashing client line buffers with "inbound JSON-RPC frame exceeded maximum line length".
pub(crate) fn bound_serialized_response(
    mut resp: serde_json::Value,
    max_frame_bytes: usize,
) -> String {
    let Ok(initial) = serde_json::to_string(&resp) else {
        return serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32603, "message": "Failed to serialize response" }
        })
        .to_string();
    };
    if initial.len() <= max_frame_bytes {
        return initial;
    }
    // Only apply tool-content truncation to tool-call results (which have `result.content` text blocks).
    // Non-tool responses (such as `tools/list`, `initialize`, etc.) must not have their response
    // structure replaced with a tool-call-shaped content array.
    let text_blocks: Vec<(usize, String)> = resp
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, b)| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text")
                            .and_then(|t| t.as_str())
                            .map(|s| (i, s.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    if text_blocks.is_empty() {
        let err_resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": resp.get("id").cloned().unwrap_or(serde_json::Value::Null),
            "error": {
                "code": -32000,
                "message": "Response exceeds maximum JSON-RPC frame size"
            }
        });
        if let Ok(serialized) = serde_json::to_string(&err_resp)
            && serialized.len() <= max_frame_bytes
        {
            return serialized;
        }
        return serde_json::json!({
            "jsonrpc": "2.0",
            "id": serde_json::Value::Null,
            "error": {
                "code": -32000,
                "message": "Response exceeds maximum JSON-RPC frame size"
            }
        })
        .to_string();
    }

    let marker = "\n[... output truncated to avoid exceeding MCP frame line limits]\n";
    for (idx, text) in text_blocks {
        let mut target_len = max_frame_bytes / 2;
        while target_len > 100 {
            let mut candidate_text =
                crate::verify::truncate_to_boundary(&text, target_len).to_string();
            candidate_text.push_str(marker);
            if let Some(block) = resp
                .get_mut("result")
                .and_then(|r| r.get_mut("content"))
                .and_then(|c| c.as_array_mut())
                .and_then(|arr| arr.get_mut(idx))
            {
                block["text"] = serde_json::Value::String(candidate_text);
            }
            if let Ok(serialized) = serde_json::to_string(&resp)
                && serialized.len() <= max_frame_bytes
            {
                return serialized;
            }
            target_len = target_len.saturating_sub(4096);
        }
    }
    let fallback = serde_json::json!({
        "jsonrpc": "2.0",
        "id": resp.get("id"),
        "result": {
            "content": [{
                "type": "text",
                "text": "[... output truncated to avoid exceeding MCP frame line limits]\n"
            }],
            "isError": false
        }
    });
    if let Ok(serialized) = serde_json::to_string(&fallback)
        && serialized.len() <= max_frame_bytes
    {
        return serialized;
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "result": {
            "content": [{
                "type": "text",
                "text": "[... output truncated to avoid exceeding MCP frame line limits]\n"
            }],
            "isError": false
        }
    })
    .to_string()
}
