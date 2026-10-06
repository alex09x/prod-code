/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::encoding::POSITION_ENCODING_UTF16;
use super::reconnect::replay_lsp_workspace_state;
use super::reconnect_handler::{handle_reconnect, ReconnectContext};
use super::session::open_editor_session;
use super::state::{fail_pending_requests, LspStateTracker};
use super::sync::{keep_checkout_synced, push_checkout};
use super::transport::{
    lsp_trace, refuse_lsp, spawn_editor_frame_reader, spawn_editor_stdout_task, trace_message,
    PendingRequests,
};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::WireMessage;
use std::collections::VecDeque;
use std::env;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn run_lsp_bridge(
    mut remote: SocketAddr,
    engine: Option<&'static str>,
    reconnect: bool,
    watchdog_secs: u64,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let cwd_str = cwd.to_string_lossy().to_string();
    let identity = prod_code_mcp::sync::workspace_identity(&cwd);

    let (framed, handshake_resp, effective_remote) =
        match open_editor_session(remote, engine, &cwd, cwd_str.clone(), identity.clone(), 0).await {
            Ok(session) => session,
            Err(err) => return refuse_lsp(&err).await,
        };
    remote = effective_remote;

    tracing::debug!(
        session_id = handshake_resp.session_id,
        engine = handshake_resp.detected_engine,
        "Connected to remote gateway"
    );
    prod_code_mcp::sync::resend_lost_files(
        &cwd,
        &prod_code_mcp::sync::gateway_node(&framed),
        &handshake_resp.stale_paths,
    );

    let files = Arc::new(prod_code_client::editor_files::RemoteFiles::new(
        remote,
        &cwd,
        Path::new(&handshake_resp.server_workspace_root),
        &prod_code_client::editor_files::default_cache(),
    ));
    let trace = lsp_trace();
    let pushing = Arc::new(Mutex::new(()));
    let (mut socket_tx, socket_rx) = framed.split();
    let mut keeper = tokio::spawn(keep_checkout_synced(
        remote,
        cwd.clone(),
        Arc::clone(&pushing),
    ));

    let pending_requests: PendingRequests = Arc::new(Mutex::new(Vec::new()));
    let mut outstanding_ping = Arc::new(AtomicBool::new(false));
    let position_encoding = Arc::new(AtomicU8::new(POSITION_ENCODING_UTF16));

    let editor_out = Arc::new(Mutex::new(tokio::io::stdout()));
    let (mut stdout_task, mut closed_rx) = spawn_editor_stdout_task(
        socket_rx,
        Arc::clone(&files),
        trace.clone(),
        identity.clone(),
        Arc::clone(&editor_out),
        Arc::clone(&pending_requests),
        Arc::clone(&outstanding_ping),
        Arc::clone(&position_encoding),
    );

    let mut tracker = LspStateTracker::default();
    let mut replay_state_after_initialize = false;
    let (editor_frame_reader, editor_frames) = spawn_editor_frame_reader();
    let mut deferred_editor_frames = VecDeque::new();

    let mut watchdog_interval = if watchdog_secs > 0 {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(watchdog_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        Some(interval)
    } else {
        None
    };

    loop {
        let frame = if let Some(frame) = deferred_editor_frames.pop_front() {
            Some(frame)
        } else {
            let mut ctx = ReconnectContext {
                engine,
                cwd: &cwd,
                cwd_str: &cwd_str,
                identity: &identity,
                tracker: &tracker,
                files: &files,
                pending_requests: &pending_requests,
                editor_out: &editor_out,
                position_encoding: &position_encoding,
                editor_frames: &editor_frames,
                deferred_editor_frames: &mut deferred_editor_frames,
                trace: &trace,
                pushing: &pushing,
            };

            tokio::select! {
                frame = async { editor_frames.lock().await.recv().await } => {
                    match frame {
                        Some(Ok(frame)) => frame,
                        Some(Err(error)) => return Err(anyhow::anyhow!("reading the editor's message: {error}")),
                        None => None,
                    }
                }
                closed = &mut closed_rx => match closed {
                    Ok((why, redirect_target)) => {
                        let is_redirect = redirect_target.is_some();
                        if reconnect {
                            let reconnect_remote = redirect_target.unwrap_or(remote);
                            handle_reconnect(
                                &mut ctx,
                                reconnect_remote,
                                is_redirect,
                                &why,
                                &mut remote,
                                &mut keeper,
                                &mut socket_tx,
                                &mut stdout_task,
                                &mut closed_rx,
                                &mut replay_state_after_initialize,
                                &mut outstanding_ping,
                            ).await;
                            continue;
                        } else {
                            fail_pending_requests(&pending_requests, &editor_out, &why).await;
                            keeper.abort();
                            eprintln!("prod-code lsp: the gateway at {remote} {why}");
                            std::process::exit(1);
                        }
                    }
                    Err(_) => break,
                },
                _ = async {
                    match &mut watchdog_interval {
                        Some(interval) => interval.tick().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if outstanding_ping.load(Ordering::Acquire) {
                        let err_msg = format!("did not respond to watchdog ping within {watchdog_secs}s");
                        if reconnect {
                            stdout_task.abort();
                            handle_reconnect(
                                &mut ctx,
                                remote,
                                false,
                                &err_msg,
                                &mut remote,
                                &mut keeper,
                                &mut socket_tx,
                                &mut stdout_task,
                                &mut closed_rx,
                                &mut replay_state_after_initialize,
                                &mut outstanding_ping,
                            ).await;
                            continue;
                        } else {
                            fail_pending_requests(&pending_requests, &editor_out, &err_msg).await;
                            keeper.abort();
                            eprintln!("prod-code lsp: the gateway at {remote} {err_msg}");
                            std::process::exit(1);
                        }
                    }

                    outstanding_ping.store(true, Ordering::Release);
                    if let Err(err) = socket_tx.send(WireMessage::Ping).await {
                        if reconnect {
                            stdout_task.abort();
                            let why = format!("broke the connection during watchdog ping: {err}");
                            handle_reconnect(
                                &mut ctx,
                                remote,
                                false,
                                &why,
                                &mut remote,
                                &mut keeper,
                                &mut socket_tx,
                                &mut stdout_task,
                                &mut closed_rx,
                                &mut replay_state_after_initialize,
                                &mut outstanding_ping,
                            ).await;
                            continue;
                        } else {
                            fail_pending_requests(&pending_requests, &editor_out, &err.to_string()).await;
                            keeper.abort();
                            eprintln!("prod-code lsp: the gateway at {remote} broke the connection: {err}");
                            std::process::exit(1);
                        }
                    }
                    continue;
                }
            }
        };

        let Some(json_payload) = frame else {
            let _ = socket_tx
                .send(WireMessage::Disconnect {
                    reason: "stdin EOF".to_string(),
                })
                .await;
            break;
        };

        let client_method = prod_code_client::editor_files::method_of(&json_payload);
        tracker.record_client_message(
            &json_payload,
            position_encoding.load(Ordering::Acquire),
        );
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_payload) {
            if let (Some(id), Some(method)) =
                (val.get("id"), val.get("method").and_then(|m| m.as_str()))
            {
                if !method.starts_with("prod-code/") {
                    pending_requests.lock().await.push((
                        id.clone(),
                        method.to_string(),
                        json_payload.clone(),
                    ));
                }
            } else if val.get("method").and_then(|m| m.as_str()) == Some("$/cancelRequest") {
                if let Some(cancel_id) = val.pointer("/params/id") {
                    pending_requests
                        .lock()
                        .await
                        .retain(|(p_id, _, _)| p_id != cancel_id);
                }
            }
        }
        trace_message(&trace, "->", &json_payload);

        if matches!(
            client_method.as_deref(),
            Some("textDocument/didSave" | "workspace/didChangeWatchedFiles")
        ) {
            let generation = prod_code_mcp::watch::current_generation(&cwd);
            let pushed = {
                let _one_at_a_time = pushing.lock().await;
                push_checkout(remote, &cwd).await
            };
            match pushed {
                Ok(()) => prod_code_mcp::watch::mark_synced(&cwd, generation),
                Err(err) => {
                    tracing::debug!(error = %format!("{err:#}"), "sync before a save failed");
                    let warning = prod_code_client::editor_files::push_failed_warning(&err);
                    trace_message(&trace, "<-", &warning);
                    let mut stdout = editor_out.lock().await;
                    let _ =
                        prod_code_client::editor_files::write_frame(&mut *stdout, &warning).await;
                }
            }
        }

        if let Err(err) = socket_tx
            .send(WireMessage::LspPayload(files.to_node(&json_payload)))
            .await
        {
            if reconnect {
                stdout_task.abort();
                let why = format!("broke the connection: {err}");
                let mut ctx = ReconnectContext {
                    engine,
                    cwd: &cwd,
                    cwd_str: &cwd_str,
                    identity: &identity,
                    tracker: &tracker,
                    files: &files,
                    pending_requests: &pending_requests,
                    editor_out: &editor_out,
                    position_encoding: &position_encoding,
                    editor_frames: &editor_frames,
                    deferred_editor_frames: &mut deferred_editor_frames,
                    trace: &trace,
                    pushing: &pushing,
                };
                handle_reconnect(
                    &mut ctx,
                    remote,
                    false,
                    &why,
                    &mut remote,
                    &mut keeper,
                    &mut socket_tx,
                    &mut stdout_task,
                    &mut closed_rx,
                    &mut replay_state_after_initialize,
                    &mut outstanding_ping,
                )
                .await;
                if matches!(
                    client_method.as_deref(),
                    Some("textDocument/didSave" | "workspace/didChangeWatchedFiles")
                ) {
                    socket_tx
                        .send(WireMessage::LspPayload(files.to_node(&json_payload)))
                        .await
                        .context("failed to replay save/watch notification after reconnect")?;
                }
            } else {
                fail_pending_requests(&pending_requests, &editor_out, &err.to_string()).await;
                keeper.abort();
                eprintln!("prod-code lsp: the gateway at {remote} broke the connection: {err}");
                std::process::exit(1);
            }
        }
        if client_method.as_deref() == Some("initialized") && replay_state_after_initialize {
            replay_lsp_workspace_state(&mut socket_tx, &tracker, &files).await?;
            replay_state_after_initialize = false;
        }
    }

    editor_frame_reader.abort();
    keeper.abort();
    let _ = stdout_task.await;
    Ok(())
}
