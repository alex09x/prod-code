/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdout};
use tokio::sync::{Notify, broadcast};
use tokio::time::Instant;

use super::probe::{
    HealthProbePending, ProbeState, health_probe_sequence, lock_unpoisoned,
    record_dispatch_liveness, valid_dispatch_response,
};
use super::request_history::{IssuedRequestHistory, OwnedTask};
use super::writer::FrameWriter;

pub(crate) fn spawn_reader_loop(
    stdout: ChildStdout,
    tx: broadcast::Sender<String>,
    is_alive: Arc<AtomicBool>,
    closed: Arc<Notify>,
    child: Weak<StdMutex<Child>>,
    ordinary_epoch: Arc<AtomicU64>,
    last_activity: Arc<StdMutex<Instant>>,
    probe_state: Arc<StdMutex<ProbeState>>,
    next_probe_id: Arc<AtomicU64>,
    health_probe_id_prefix: Arc<str>,
    health_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    issued_requests: Arc<StdMutex<IssuedRequestHistory>>,
    writer: FrameWriter,
) -> OwnedTask {
    OwnedTask(tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        loop {
            let json = match prod_code_protocol::transport::read_lsp_frame(&mut reader).await {
                Ok(Some(json)) => json,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "Invalid backend LSP frame");
                    break;
                }
            };
            // Auto-respond to server-initiated requests
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                let id = val.get("id");
                let method = val.get("method").and_then(|m| m.as_str());
                let probe_identity_sequence = id
                    .and_then(|id| health_probe_sequence(id, &health_probe_id_prefix))
                    .filter(|sequence| *sequence < next_probe_id.load(Ordering::Acquire));
                let probe_sequence = method
                    .is_none()
                    .then_some(probe_identity_sequence)
                    .flatten();
                if method.is_none()
                    && let (Some(id), Some(sequence)) =
                        (id.and_then(serde_json::Value::as_str), probe_sequence)
                {
                    if valid_dispatch_response(&val) {
                        let response = {
                            let mut pending = lock_unpoisoned(&health_pending);
                            if pending.as_ref().is_some_and(|slot| slot.id == id) {
                                pending.take().map(|slot| slot.response)
                            } else {
                                None
                            }
                        };
                        let mut state = lock_unpoisoned(&probe_state);
                        if sequence > state.latest_valid_sequence {
                            state.latest_valid_sequence = sequence;
                            state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
                            state.consecutive_timeouts = 0;
                            state.valid_completions += 1;
                        }
                        drop(state);
                        if let Some(response) = response {
                            let _ = response.send(val);
                        }
                    }
                    // Every issued private identity remains classifiable from this worker's
                    // private namespace after its one pending slot is cleared. Invalid and
                    // late probe replies therefore never leak into editor subscriptions.
                    continue;
                }

                if method.is_none() {
                    if valid_dispatch_response(&val)
                        && id.is_some_and(|id| lock_unpoisoned(&issued_requests).contains(id))
                    {
                        record_dispatch_liveness(&ordinary_epoch, &last_activity, &probe_state);
                    }
                } else if probe_identity_sequence.is_none()
                    && val.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0")
                {
                    // Genuine server requests, progress and indexing notifications defer
                    // idle probes. A server request reusing a private probe id does not.
                    record_dispatch_liveness(&ordinary_epoch, &last_activity, &probe_state);
                }
                match (id, method) {
                    (
                        Some(id),
                        Some("window/workDoneProgress/create" | "client/registerCapability"),
                    ) => {
                        let auto_resp = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": null
                        })
                        .to_string();
                        if let Err(error) = writer.send_control(&auto_resp).await {
                            tracing::warn!(%error, "Failed to write automatic backend LSP response");
                            // No caller can retry this mandatory response. Keeping a server
                            // waiting forever would leave a falsely reusable backend.
                            writer.retire();
                            break;
                        }
                    }
                    (Some(id), Some("workspace/configuration")) => {
                        let items = val
                            .get("params")
                            .and_then(|params| params.get("items"))
                            .and_then(serde_json::Value::as_array)
                            .filter(|items| {
                                items.iter().all(|item| {
                                    item.is_object()
                                        && ["section", "scopeUri"].iter().all(|key| {
                                            item.get(key).is_none_or(|value| value.is_string())
                                        })
                                })
                            });
                        let auto_resp = match items {
                            Some(items) => serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": vec![serde_json::json!({}); items.len()]
                            }),
                            None => serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": {
                                    "code": -32602,
                                    "message": "workspace/configuration requires an items array of configuration objects with optional string section and scopeUri"
                                }
                            }),
                        }
                        .to_string();
                        if let Err(error) = writer.send_control(&auto_resp).await {
                            tracing::warn!(%error, "Failed to write automatic backend LSP response");
                            // No caller can retry this mandatory response. Keeping a server
                            // waiting forever would leave a falsely reusable backend.
                            writer.retire();
                            break;
                        }
                    }
                    _ => {}
                }
                if probe_sequence.is_some() {
                    continue;
                }
            }

            // Broadcast frame to all connected sessions
            let _ = tx.send(json);
        }
        is_alive.store(false, Ordering::Release);
        closed.notify_waiters();
        closed.notify_one();
        if let Some(child) = child.upgrade() {
            match child.lock() {
                Ok(mut child) => {
                    let _ = child.start_kill();
                }
                Err(error) => {
                    let mut child = error.into_inner();
                    let _ = child.start_kill();
                }
            }
        }
        tracing::info!("Backend worker reader loop terminated");
    }))
}
