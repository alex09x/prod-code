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
use crate::server::reload::{ActiveCalls, Reload, ToolCalls, deliver_response, drain_for_reload};

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

    // Watch the installed executable for reloads between requests.
    let exe = std::env::current_exe().ok();
    let (stdin, reload) = super::stdin::open(exe).context("Failed opening stdin")?;
    serve_with_reload(
        remote,
        workspace_root,
        resumed,
        BufReader::new(stdin),
        tokio::io::stdout(),
        reload,
    )
    .await
}

/// Serve MCP requests over a testable transport. `exe: None` disables hot reload.
/// Arbitrary AsyncRead inputs may own bytes after Pending; reload waits for their result.
pub async fn serve_mcp_requests<R, W>(
    remote: SocketAddr,
    workspace_root: PathBuf,
    exe: Option<PathBuf>,
    resumed: bool,
    reader: BufReader<R>,
    writer: W,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    serve_with_reload(
        remote,
        workspace_root,
        resumed,
        reader,
        writer,
        Reload::new(exe),
    )
    .await
}

pub(super) async fn serve_with_reload<R, W>(
    mut remote: SocketAddr,
    workspace_root: PathBuf,
    resumed: bool,
    mut reader: BufReader<R>,
    mut writer: W,
    mut reload: Reload,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    if resumed {
        writer
            .write_all(hot_reload::tools_list_changed().as_bytes())
            .await?;
        writer.flush().await?;
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut eof = false;
    let mut read_pending = false;
    let mut in_flight = ToolCalls::new();
    let mut active_aborts = ActiveCalls::new();
    let mut reload_deadline = None;
    let period = reload.rebalance_every;
    let mut rebalance_interval =
        tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    rebalance_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if reload.requested.load(std::sync::atomic::Ordering::Acquire)
            && let Some(exe) = &reload.exe
        {
            let deadline = *reload_deadline
                .get_or_insert_with(|| tokio::time::Instant::now() + reload.timeout);
            drain_for_reload(
                &mut in_flight,
                &mut active_aborts,
                &mut remote,
                &mut writer,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await?;
            // Bytes already read from stdin cannot be recovered by exec. Finish the input
            // frame before swapping, without granting newly dispatched calls another budget.
            if pending.is_empty()
                && reader.buffer().is_empty()
                && (!read_pending || reload.cancel_safe_input)
            {
                writer
                    .write_all(hot_reload::tools_list_changed().as_bytes())
                    .await?;
                writer.flush().await?;
                tracing::info!(exe = %exe.display(), "re-executing the installed binary");
                let err = (reload.reexec)(exe);
                tracing::error!(error = %err, "hot reload failed; continuing with the current binary");
                reload
                    .requested
                    .store(false, std::sync::atomic::Ordering::Release);
                reload_deadline = None;
            }
        }

        if eof && in_flight.is_empty() {
            break;
        }
        let consumed = tokio::select! {
            filled = super::stdin::fill(&mut reader, &mut pending, &mut read_pending), if !eof => {
                let count = filled.context("Failed reading from stdin")?;
                eof = count == 0;
                count
            }
            Some(joined) = in_flight.join_next(), if !in_flight.is_empty() => {
                deliver_response(joined, &mut active_aborts, &mut remote, &mut writer).await?;
                0
            }
            _ = reload.notify.notified(), if !eof => 0,
            _ = rebalance_interval.tick(), if !eof => {
                crate::server::rebalance::handle_cluster_rebalance_tick(
                    &mut remote,
                    &workspace_root,
                )
                .await;
                0
            }
        };
        if consumed > 0 {
            reader.consume(consumed);
        }

        while let Some(line) = hot_reload::take_line(&mut pending) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.len() > MAX_JSONRPC_FRAME_BYTES {
                let err_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": serde_json::Value::Null,
                    "error": { "code": -32600, "message": "Request exceeds maximum JSON-RPC frame size" }
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
                        "id": serde_json::Value::Null,
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
            let method = req_json
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            if method == "notifications/cancelled" || method == "$/cancelRequest" {
                if let Some(cancel_id) = req_json
                    .pointer("/params/requestId")
                    .or_else(|| req_json.pointer("/params/id"))
                    && let Ok(key) = serde_json::to_string(cancel_id)
                    && let Some(handle) = active_aborts.remove(&key)
                {
                    handle.abort();
                }
                continue;
            }
            let req_id = req_json.get("id").cloned();
            if method == "tools/call" {
                let id_key = req_id
                    .as_ref()
                    .and_then(|id| serde_json::to_string(id).ok());
                let id_key_for_task = id_key.clone();
                let start_remote = remote;
                let mut task_remote = start_remote;
                let ws_root = workspace_root.clone();
                let abort_handle = in_flight.spawn(async move {
                    let fut = std::panic::AssertUnwindSafe(handle_mcp_request(
                        &mut task_remote,
                        &ws_root,
                        req_json,
                    ));
                    let outcome = fut.catch_unwind().await;
                    let changed_remote = (task_remote != start_remote).then_some(task_remote);
                    (
                        id_key_for_task,
                        changed_remote,
                        format_handled_response(req_id, outcome),
                    )
                });
                if let Some(k) = id_key {
                    active_aborts.insert(k, abort_handle);
                }
            } else {
                let fut = std::panic::AssertUnwindSafe(handle_mcp_request(
                    &mut remote,
                    &workspace_root,
                    req_json,
                ));
                if let Some(out) = format_handled_response(req_id, fut.catch_unwind().await)
                    && (writer.write_all(out.as_bytes()).await.is_err()
                        || writer.flush().await.is_err())
                {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

pub(super) fn format_handled_response(
    req_id: Option<serde_json::Value>,
    outcome: std::result::Result<Result<Option<serde_json::Value>>, Box<dyn std::any::Any + Send>>,
) -> Option<String> {
    match outcome {
        Ok(Ok(Some(resp_val))) => {
            let mut out = bound_serialized_response(resp_val, MAX_JSONRPC_FRAME_BYTES);
            out.push('\n');
            Some(out)
        }
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "MCP request handler internal error");
            let err_resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id.unwrap_or(serde_json::Value::Null),
                "error": { "code": -32603, "message": format!("Internal error: {e}") }
            });
            serde_json::to_string(&err_resp).ok().map(|mut s| {
                s.push('\n');
                s
            })
        }
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(panic = %msg, "MCP request handler panicked");
            let err_resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": { "code": -32603, "message": format!("Internal error (panic): {msg}") }
            });
            serde_json::to_string(&err_resp).ok().map(|mut s| {
                s.push('\n');
                s
            })
        }
    }
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
        for id in [
            resp.get("id").cloned().unwrap_or(serde_json::Value::Null),
            serde_json::Value::Null,
        ] {
            let err_resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": "Response exceeds maximum JSON-RPC frame size" }
            });
            if let Ok(s) = serde_json::to_string(&err_resp)
                && s.len() <= max_frame_bytes
            {
                return s;
            }
        }
        return r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32000,"message":"Response exceeds maximum JSON-RPC frame size"}}"#.to_string();
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
    for id in [
        resp.get("id").cloned().unwrap_or(serde_json::Value::Null),
        serde_json::Value::Null,
    ] {
        let fallback = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": marker.trim_start() }], "isError": false }
        });
        if let Ok(s) = serde_json::to_string(&fallback)
            && s.len() <= max_frame_bytes
        {
            return s;
        }
    }
    r#"{"jsonrpc":"2.0","id":null,"result":{"content":[{"type":"text","text":"[... output truncated to avoid exceeding MCP frame line limits]\n"}],"isError":false}}"#.to_string()
}
