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
use crate::server::dispatch::handle_mcp_request_at;
use crate::server::rebalance_task::Rebalancer;
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
    // The tick's status probes run beside the loop, never in it (#1814).
    let mut rebalancer = Rebalancer::default();

    loop {
        if reload.requested.load(std::sync::atomic::Ordering::Acquire)
            && let Some(exe) = &reload.exe
        {
            let deadline = *reload_deadline
                .get_or_insert_with(|| tokio::time::Instant::now() + reload.timeout);
            // Keep reading during the drain: unread calls would otherwise receive a fresh
            // navigation budget only after the old calls finish (#1814).
            if tokio::time::Instant::now() >= deadline {
                drain_for_reload(
                    &mut in_flight,
                    &mut active_aborts,
                    &mut remote,
                    &mut writer,
                    std::time::Duration::ZERO,
                )
                .await?;
            }
            // Bytes already read from stdin cannot be recovered by exec. Finish the input
            // frame before swapping, without granting newly dispatched calls another budget.
            if in_flight.is_empty()
                && pending.is_empty()
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
            _ = tokio::time::sleep_until(reload_deadline.unwrap_or_else(tokio::time::Instant::now)),
                if reload_deadline.is_some() && !in_flight.is_empty() => 0,
            _ = reload.notify.notified(), if !eof => 0,
            _ = rebalance_interval.tick(), if !eof => {
                rebalancer.start(remote, &workspace_root);
                0
            }
            checked = rebalancer.finished(), if rebalancer.is_running() => {
                Rebalancer::apply(&mut remote, checked, &workspace_root);
                0
            }
        };
        if consumed > 0 {
            reader.consume(consumed);
        }
        // A reload requested while the loop waited is in force for what that wait just read: the
        // top of the loop has not seen it yet, and without this a request read in the same wakeup
        // was served as ordinary work instead of being refused for the drain.
        if reload.requested.load(std::sync::atomic::Ordering::Acquire) && reload.exe.is_some() {
            reload_deadline.get_or_insert_with(|| tokio::time::Instant::now() + reload.timeout);
        }

        while let Some(line) = hot_reload::take_line(&mut pending) {
            // A request's budgets count from here, through every retry (#1814).
            let received = tokio::time::Instant::now();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.len() > MAX_JSONRPC_FRAME_BYTES {
                let message = "Request exceeds maximum JSON-RPC frame size";
                write_frame_error(&mut writer, -32600, message).await;
                continue;
            }
            let req_json = match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    write_frame_error(&mut writer, -32700, &format!("Parse error: {e}")).await;
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
            if reload_deadline.is_some()
                && super::reload::refuse_new_work(&req_json, &mut writer).await?
            {
                continue;
            }
            if method == "tools/call" {
                let id_key = req_id
                    .as_ref()
                    .and_then(|id| serde_json::to_string(id).ok());
                let id_key_for_task = id_key.clone();
                let start_remote = remote;
                let mut task_remote = start_remote;
                let ws_root = workspace_root.clone();
                let abort_handle = in_flight.spawn(async move {
                    let fut = std::panic::AssertUnwindSafe(handle_mcp_request_at(
                        &mut task_remote,
                        &ws_root,
                        req_json,
                        received,
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
                let fut = std::panic::AssertUnwindSafe(handle_mcp_request_at(
                    &mut remote,
                    &workspace_root,
                    req_json,
                    received,
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

/// Answers a frame that is not a request with a JSON-RPC error without an id; a write failure is
/// left to the next response to notice.
async fn write_frame_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    code: i64,
    message: &str,
) {
    if let Some(out) = error_line(None, code, message) {
        let _ = writer.write_all(out.as_bytes()).await;
        let _ = writer.flush().await;
    }
}

/// One JSON-RPC error response line for `id` (`null` when there is none).
fn error_line(id: Option<serde_json::Value>, code: i64, message: &str) -> Option<String> {
    let err_resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    serde_json::to_string(&err_resp).ok().map(|out| out + "\n")
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
            tracing::error!(
                error = format!("{e:#}"),
                "MCP request handler internal error"
            );
            error_line(req_id, -32603, &format!("Internal error: {e:#}"))
        }
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(panic = %msg, "MCP request handler panicked");
            error_line(req_id, -32603, &format!("Internal error (panic): {msg}"))
        }
    }
}

pub const MAX_JSONRPC_FRAME_BYTES: usize = 512 * 1024;

// Keep this import path stable for existing users and function-local test imports.
pub(crate) use super::frames::bound_serialized_response;
