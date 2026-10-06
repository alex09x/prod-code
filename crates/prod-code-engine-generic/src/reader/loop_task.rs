/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

use super::frame::write_frame_until;
use crate::config::discovery::settings_for_section;
use crate::diagnostics::{Published, Sent};
use crate::types::{
    FrameWrite, HealthProbePending, ProbeState, health_probe_sequence, lock_unpoisoned,
    valid_health_response,
};
use prod_code_protocol::readiness::Readiness;

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_reader_loop(
    stdout: ChildStdout,
    readiness: Arc<Readiness>,
    pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    next_probe_id: Arc<AtomicU64>,
    health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    probe_state: Arc<StdMutex<ProbeState>>,
    activity: Arc<RwLock<Instant>>,
    config_root: PathBuf,
    apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    auto_reply_timeout: Duration,
    child_weak: Weak<StdMutex<Child>>,
    is_alive: Arc<AtomicBool>,
    sent: Arc<RwLock<HashMap<String, Sent>>>,
    diagnostics: Arc<RwLock<HashMap<String, Published>>>,
    versioned: Arc<AtomicBool>,
    bcast_tx: broadcast::Sender<String>,
    accepts_documents: Arc<AtomicBool>,
    capabilities: Arc<RwLock<Option<serde_json::Value>>>,
) {
    let pending_writer = Arc::downgrade(&pending_requests);
    let is_alive_clone = Arc::clone(&is_alive);
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
                readiness.on_message(&val);
                if let Some(id_val) = val.get("id") {
                    // Only an answer is ours: a request from the server
                    // (`window/workDoneProgress/create`) numbers its own ids
                    // from 1 too, and taken for the answer to ours it left
                    // the server waiting and our question empty (#391).
                    if val.get("method").is_none()
                        && let Some(id) = id_val.as_u64()
                    {
                        let tx = lock_unpoisoned(&pending_requests).remove(&id);
                        if let Some(tx) = tx {
                            lock_unpoisoned(&probe_state).consecutive_timeouts = 0;
                            let mut act = activity.write().await;
                            *act = Instant::now();
                            let _ = tx.send(val.clone());
                            continue;
                        }
                    } else if val.get("method").is_none()
                        && let Some(sequence) =
                            health_probe_sequence(&val, next_probe_id.load(Ordering::Acquire))
                    {
                        let id = id_val.as_str().expect("health ids are strings");
                        let response = {
                            let mut pending = lock_unpoisoned(&health_probe_pending);
                            if pending.as_ref().is_some_and(|slot| slot.id == id) {
                                pending.take().map(|slot| slot.response)
                            } else {
                                None
                            }
                        };
                        if valid_health_response(&val, id) {
                            let mut state = lock_unpoisoned(&probe_state);
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
                        // Issued string ids remain classifiable after their bounded slot
                        // is cleared, so no late probe response reaches subscribers.
                        continue;
                    }

                    // Auto-respond to server requests
                    let method = val.get("method").and_then(|m| m.as_str());
                    if let Some(m) = method {
                        let resp = match m {
                            "window/workDoneProgress/create" | "client/registerCapability" => {
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "result": null
                                })
                            }
                            "workspace/configuration" => {
                                let sections: Vec<String> = val
                                    .get("params")
                                    .and_then(|p| p.get("items"))
                                    .and_then(|i| i.as_array())
                                    .map(|items| {
                                        items
                                            .iter()
                                            .map(|item| {
                                                item.get("section")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("")
                                                    .to_string()
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_else(|| vec![String::new()]);
                                let values: Vec<serde_json::Value> = sections
                                    .iter()
                                    .map(|section| settings_for_section(&config_root, section))
                                    .collect();
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "result": values
                                })
                            }
                            "workspace/applyEdit" => {
                                let edit = val
                                    .get("params")
                                    .and_then(|p| p.get("edit"))
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                if let Some(tx) = apply_edit_waiter.lock().await.take() {
                                    let _ = tx.send(edit);
                                }
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "result": { "applied": true }
                                })
                            }
                            "workspace/workspaceFolders" => {
                                let ws_uri = url::Url::from_directory_path(&config_root)
                                    .map(|u| u.to_string())
                                    .unwrap_or_else(|_| {
                                        url::Url::from_file_path(&config_root)
                                            .map(|u| u.to_string())
                                            .unwrap_or_else(|_| {
                                                format!("file://{}", config_root.display())
                                            })
                                    });
                                let ws_name = config_root
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("generic-workspace");
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "result": [
                                        {
                                            "name": ws_name,
                                            "uri": ws_uri
                                        }
                                    ]
                                })
                            }
                            other => {
                                tracing::debug!(
                                    method = other,
                                    "unsupported server request refused"
                                );
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_val,
                                    "error": { "code": -32601, "message": format!("{other} is not supported by prod-code") }
                                })
                            }
                        };
                        if let Err(error) = write_frame_until(
                            &stdin,
                            &resp,
                            tokio::time::Instant::now() + auto_reply_timeout,
                            m,
                            &child_weak,
                            &pending_writer,
                            &Arc::downgrade(&is_alive_clone),
                        )
                        .await
                        {
                            tracing::warn!(
                                method = m,
                                error = %error,
                                "failed to answer language-server request; retiring process"
                            );
                            let mut retirement = FrameWrite {
                                child: child_weak.clone(),
                                pending: pending_writer.clone(),
                                is_alive: Arc::downgrade(&is_alive_clone),
                                started: true,
                                complete: false,
                            };
                            retirement.retire();
                            retirement.complete = true;
                            break 'reader;
                        }
                    }
                }

                {
                    let mut act = activity.write().await;
                    *act = Instant::now();
                }

                if val.get("method").and_then(|m| m.as_str())
                    == Some("textDocument/publishDiagnostics")
                    && let Some(uri) = val
                        .get("params")
                        .and_then(|p| p.get("uri"))
                        .and_then(|u| u.as_str())
                {
                    let items = val
                        .get("params")
                        .and_then(|p| p.get("diagnostics"))
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let version = val.pointer("/params/version").and_then(|v| v.as_i64());
                    if version.is_some() {
                        versioned.store(true, Ordering::Relaxed);
                    }
                    let publication = Published {
                        version,
                        at: Instant::now(),
                        items,
                    };
                    let sent_version = sent.read().await.get(uri).and_then(|s| s.version);
                    let mut diags = diagnostics.write().await;
                    let is_late_older = matches!(
                        (
                            diags.get(uri).and_then(|p| p.version),
                            publication.version,
                            sent_version,
                        ),
                        (Some(current), Some(incoming), Some(sent_ver))
                            if current == sent_ver && incoming < current
                    );
                    if !is_late_older {
                        diags.insert(uri.to_string(), publication);
                    }
                }
                let _ = bcast_tx.send(json_str);
            }
        }
        is_alive_clone.store(false, Ordering::Relaxed);
        lock_unpoisoned(&pending_requests).clear();
        lock_unpoisoned(&health_probe_pending).take();
        accepts_documents.store(false, Ordering::Release);
        *capabilities.write().await = None;
        let mut exit_status = None;
        if let Some(child) = child_weak.upgrade() {
            let _ = lock_unpoisoned(&child).start_kill();
            for _ in 0..200 {
                let status = {
                    let mut child = lock_unpoisoned(&child);
                    child.try_wait()
                };
                match status {
                    Ok(Some(status)) => {
                        exit_status = Some(status);
                        break;
                    }
                    Err(_) => break,
                    Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        }
        let status_msg = match exit_status {
            Some(status) => match status.code() {
                Some(code) => format!("exit code {code}"),
                None => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        status.signal().map_or_else(
                            || "process terminated".to_string(),
                            |sig| format!("signal {sig}"),
                        )
                    }
                    #[cfg(not(unix))]
                    {
                        "process terminated".to_string()
                    }
                }
            },
            None => "process terminated".to_string(),
        };
        tracing::info!(status = %status_msg, "Generic LSP reader loop finished");
    });
}
