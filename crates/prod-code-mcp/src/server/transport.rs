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
            let handle_future = std::panic::AssertUnwindSafe(handle_mcp_request(
                &mut remote,
                &workspace_root,
                req_json,
            ));
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
