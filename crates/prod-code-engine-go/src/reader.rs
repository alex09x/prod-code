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
use prod_code_protocol::readiness::Readiness;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

use crate::types::{
    FrameWrite, HealthProbePending, ProbeState, health_probe_sequence, lock_unpoisoned,
    valid_health_response,
};

/// Helper to write an LSP Content-Length frame directly to child stdin.
pub(crate) async fn write_frame_until(
    writer: &Arc<Mutex<ChildStdin>>,
    val: &serde_json::Value,
    deadline: tokio::time::Instant,
    method: &str,
    child: &Weak<StdMutex<Child>>,
    pending: &Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    is_alive: &Weak<AtomicBool>,
) -> Result<()> {
    let body = val.to_string();
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let mut sin = tokio::time::timeout_at(deadline, writer.lock())
        .await
        .with_context(|| format!("Timeout waiting to send gopls message '{method}'"))?;
    if !is_alive
        .upgrade()
        .is_some_and(|alive| alive.load(Ordering::Acquire))
    {
        anyhow::bail!("gopls process has exited before message '{method}'");
    }
    // Declared after `sin`, so cancellation retires the process before unlocking stdin.
    let mut frame_write = FrameWrite {
        child: child.clone(),
        pending: pending.clone(),
        is_alive: is_alive.clone(),
        started: true,
        complete: false,
    };
    tokio::time::timeout_at(deadline, sin.write_all(frame.as_bytes()))
        .await
        .with_context(|| format!("Timeout writing gopls message '{method}'"))?
        .with_context(|| format!("Failed to write gopls message '{method}'"))?;
    tokio::time::timeout_at(deadline, sin.flush())
        .await
        .with_context(|| format!("Timeout flushing gopls message '{method}'"))?
        .with_context(|| format!("Failed to flush gopls message '{method}'"))?;
    frame_write.complete = true;
    Ok(())
}

pub(crate) fn spawn_reader_loop(
    stdout: ChildStdout,
    readiness_reader: Arc<Readiness>,
    pending_clone: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    probe_state_reader: Arc<StdMutex<ProbeState>>,
    next_probe_id_reader: Arc<AtomicU64>,
    health_probe_pending_reader: Arc<StdMutex<Option<HealthProbePending>>>,
    stdin_writer: Arc<Mutex<ChildStdin>>,
    child_writer: Weak<StdMutex<Child>>,
    pending_writer: Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    is_alive_reader: Arc<AtomicBool>,
    capabilities_reader: Arc<RwLock<Option<serde_json::Value>>>,
    bcast_tx_clone: broadcast::Sender<String>,
    auto_reply_timeout: Duration,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        'reader: loop {
            let json_str = match prod_code_protocol::transport::read_lsp_frame(&mut reader).await {
                Ok(Some(json)) => json,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "Invalid language-server LSP frame");
                    break;
                }
            };
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                readiness_reader.on_message(&val);
                // 1. Check if this is a response to our pending request
                if let Some(id_val) = val.get("id") {
                    // Only an answer is ours: gopls numbers its own requests
                    // (`window/workDoneProgress/create`) from 1 too (#391).
                    if val.get("method").is_none()
                        && let Some(id) = id_val.as_u64()
                    {
                        let mut pending = lock_unpoisoned(&pending_clone);
                        if let Some(tx) = pending.remove(&id) {
                            lock_unpoisoned(&probe_state_reader).consecutive_timeouts = 0;
                            let _ = tx.send(val.clone());
                            continue;
                        }
                    } else if val.get("method").is_none()
                        && let Some(sequence) = health_probe_sequence(
                            &val,
                            next_probe_id_reader.load(Ordering::Acquire),
                        )
                    {
                        let id = id_val.as_str().expect("health ids are strings");
                        let response = {
                            let mut pending = lock_unpoisoned(&health_probe_pending_reader);
                            if pending.as_ref().is_some_and(|slot| slot.id == id) {
                                pending.take().map(|slot| slot.response)
                            } else {
                                None
                            }
                        };
                        if valid_health_response(&val, id) {
                            let mut state = lock_unpoisoned(&probe_state_reader);
                            state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
                            state.consecutive_timeouts = 0;
                            if sequence > state.latest_valid_sequence {
                                state.latest_valid_sequence = sequence;
                                state.valid_completions += 1;
                            }
                        }
                        if let Some(response) = response {
                            let _ = response.send(val.clone());
                        }
                        // All issued health ids stay recognizable without retaining them,
                        // so even very late or malformed probe traffic remains private.
                        continue;
                    }

                    // Server-initiated request requiring auto-reply
                    let method = val.get("method").and_then(|m| m.as_str());
                    if let Some(m) = method {
                        let auto_resp = match m {
                            "window/workDoneProgress/create" | "client/registerCapability" => {
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "result": null
                                })
                            }
                            "workspace/configuration" => {
                                let items = val
                                    .get("params")
                                    .and_then(|params| params.get("items"))
                                    .and_then(serde_json::Value::as_array)
                                    .filter(|items| {
                                        items.iter().all(|item| {
                                            item.is_object()
                                                && ["section", "scopeUri"].iter().all(|key| {
                                                    item.get(key)
                                                        .is_none_or(|value| value.is_string())
                                                })
                                        })
                                    });
                                match items {
                                    Some(items) => serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id_val,
                                        "result": vec![serde_json::json!({}); items.len()]
                                    }),
                                    None => serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id_val,
                                        "error": {
                                            "code": -32602,
                                            "message": "workspace/configuration requires an items array of configuration objects with optional string section and scopeUri"
                                        }
                                    }),
                                }
                            }
                            _ => serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id_val,
                                "error": { "code": -32601, "message": format!("{m} is not supported by prod-code") }
                            }),
                        };
                        if let Err(error) = write_frame_until(
                            &stdin_writer,
                            &auto_resp,
                            tokio::time::Instant::now() + auto_reply_timeout,
                            m,
                            &child_writer,
                            &pending_writer,
                            &Arc::downgrade(&is_alive_reader),
                        )
                        .await
                        {
                            tracing::warn!(
                                method = m,
                                error = %error,
                                "failed to answer gopls request; retiring process"
                            );
                            let mut retirement = FrameWrite {
                                child: child_writer.clone(),
                                pending: pending_writer.clone(),
                                is_alive: Arc::downgrade(&is_alive_reader),
                                started: true,
                                complete: false,
                            };
                            retirement.retire();
                            retirement.complete = true;
                            break 'reader;
                        }
                    }
                }

                // Broadcast notification to listeners
                let _ = bcast_tx_clone.send(json_str);
            }
        }
        is_alive_reader.store(false, Ordering::Relaxed);
        // No answer is coming for a request still waiting: dropping its sender ends the
        // wait now, not at the timeout (#355).
        lock_unpoisoned(&pending_clone).clear();
        lock_unpoisoned(&health_probe_pending_reader).take();
        *capabilities_reader.write().await = None;
        if let Some(child) = child_writer.upgrade() {
            let _ = lock_unpoisoned(&child).start_kill();
            for _ in 0..200 {
                let status = {
                    let mut child = lock_unpoisoned(&child);
                    child.try_wait()
                };
                match status {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        }
        tracing::info!("gopls background reader loop stopped");
    });
}
