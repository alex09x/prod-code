/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::encoding::lsp_position_encoding;
use super::session::resolve_redirect_target;
use anyhow::Result;
use futures_util::StreamExt;
use futures_util::stream::SplitStream;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::io::BufReader;
use tokio_util::codec::Framed;

/// The file `PROD_CODE_LSP_TRACE` names, where the bridge logs every message it carries: the
/// time, the direction, the method or id, and the size.
pub type LspTrace = Option<Arc<std::sync::Mutex<std::fs::File>>>;

pub fn lsp_trace() -> LspTrace {
    let path = env::var_os("PROD_CODE_LSP_TRACE")?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    Some(Arc::new(std::sync::Mutex::new(file)))
}

pub fn trace_message(trace: &LspTrace, direction: &str, raw: &str) {
    let Some(file) = trace else {
        return;
    };
    let id = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .map(|id| id.to_string())
        .unwrap_or_default();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let method = prod_code_client::editor_files::method_of(raw).unwrap_or_default();
    if let Ok(mut file) = file.lock() {
        let line = format!("{millis} {direction} {method} id={id} {}B\n", raw.len());
        let _ = std::io::Write::write_all(&mut *file, line.as_bytes());
    }
}

/// Tells the editor why its language server could not start.
pub async fn refuse_lsp(err: &anyhow::Error) -> Result<()> {
    let message = prod_code_client::editor_files::startup_error_message(err);
    eprintln!("{message}");
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    prod_code_client::editor_files::refuse_session(&mut stdin, &mut stdout, &message).await?;
    Ok(())
}

pub type PendingRequests = Arc<tokio::sync::Mutex<Vec<(serde_json::Value, String, String)>>>;
pub type EditorFrameReceiver = Arc<
    tokio::sync::Mutex<tokio::sync::mpsc::Receiver<std::result::Result<Option<String>, String>>>,
>;
pub const MAX_DEFERRED_EDITOR_FRAMES: usize = 128;

pub fn spawn_editor_stdout_task(
    mut socket_rx: SplitStream<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>>,
    stdout_files: Arc<prod_code_client::editor_files::RemoteFiles>,
    stdout_trace: LspTrace,
    stdout_identity: prod_code_mcp::sync::WorkspaceIdentity,
    editor_out: Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
    pending_requests: PendingRequests,
    outstanding_ping: Arc<AtomicBool>,
    position_encoding: Arc<AtomicU8>,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<(String, Option<SocketAddr>)>,
) {
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<(String, Option<SocketAddr>)>();
    let handle = tokio::spawn(async move {
        let mut redirect_target = None;
        let why = loop {
            match socket_rx.next().await {
                Some(Ok(msg)) => match msg {
                    WireMessage::LspPayload(json) => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                            if let Some(encoding) = lsp_position_encoding(&val) {
                                position_encoding.store(encoding, Ordering::Release);
                            }
                            if val.get("id").is_some() && val.get("method").is_none() {
                                if let Some(id) = val.get("id") {
                                    let mut pending = pending_requests.lock().await;
                                    pending.retain(|(p_id, _, _)| p_id != id);
                                }
                            }
                        }
                        let json = stdout_files.to_editor(json).await;
                        trace_message(&stdout_trace, "<-", &json);
                        let mut stdout = editor_out.lock().await;
                        if prod_code_client::editor_files::write_frame(&mut *stdout, &json)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    WireMessage::Redirect {
                        target_addr,
                        reason,
                    } => {
                        tracing::info!(%target_addr, ?reason, "received dynamic rebalance redirect from gateway");
                        if let Ok(addr) = resolve_redirect_target(&target_addr) {
                            redirect_target = Some(addr);
                            prod_code_mcp::cluster::remember_placement(&stdout_identity.name, addr);
                        }
                        continue;
                    }
                    WireMessage::Pong => {
                        outstanding_ping.store(false, Ordering::Release);
                        tracing::trace!("received watchdog pong from gateway");
                        continue;
                    }
                    WireMessage::Disconnect { reason } => {
                        break format!("closed the session: {reason}");
                    }
                    _ => {}
                },
                Some(Err(err)) => break format!("broke the connection: {err}"),
                None => break "closed the connection".to_string(),
            }
        };
        let _ = closed_tx.send((why, redirect_target));
    });
    (handle, closed_rx)
}

pub fn spawn_editor_frame_reader() -> (tokio::task::JoinHandle<()>, EditorFrameReceiver) {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let task = tokio::spawn(async move {
        let mut reader = BufReader::new(tokio::io::stdin());
        loop {
            let frame = prod_code_client::editor_files::read_frame(&mut reader)
                .await
                .map_err(|error| error.to_string());
            let eof = matches!(frame, Ok(None));
            if tx.send(frame).await.is_err() || eof {
                break;
            }
        }
    });
    (task, Arc::new(tokio::sync::Mutex::new(rx)))
}
