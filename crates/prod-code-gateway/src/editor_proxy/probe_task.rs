/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::readiness::Readiness;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::Instant;

use super::probe::{HEALTH_PROBE_METHOD, HealthProbePending, MAX_IDLE_PROBE_TIMEOUTS, ProbeState};
use super::registry::PendingServerFrame;

pub(crate) fn spawn_probe_task(
    interval: Duration,
    response_timeout: Duration,
    session_id: u64,
    to_server_tx: rapidfire::mpsc::Sender<PendingServerFrame>,
    health_pending: Arc<std::sync::Mutex<Option<HealthProbePending>>>,
    probe_state: Arc<std::sync::Mutex<ProbeState>>,
    next_probe_id: Arc<AtomicU64>,
    health_probe_id_prefix: Arc<str>,
    last_activity: Arc<std::sync::Mutex<Instant>>,
    probe_ordinary_epoch: Arc<AtomicU64>,
    probe_lsp_initialized: Arc<AtomicBool>,
    readiness: Arc<Readiness>,
    in_flight_requests: Arc<AtomicUsize>,
    retire_tx: tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if *retire_tx.borrow() {
                break;
            }

            if !probe_lsp_initialized.load(Ordering::Acquire) {
                continue;
            }

            let activity_before_wait = probe_ordinary_epoch.load(Ordering::Acquire);
            let evidence_before_wait = probe_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .valid_evidence_epoch;

            // Loaded-project stress policy:
            // 1. In-flight requests from editor: server is actively handling requests
            if in_flight_requests.load(Ordering::Acquire) > 0 {
                continue;
            }

            // 2. Server readiness: indexing / loading in progress
            if readiness.busy().is_some() {
                continue;
            }

            // 3. Traffic occurred recently
            let elapsed = last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .elapsed();
            if elapsed < interval {
                continue;
            }

            // Re-verify that no concurrent activity or in-flight requests occurred
            if in_flight_requests.load(Ordering::Acquire) > 0
                || probe_ordinary_epoch.load(Ordering::Acquire) != activity_before_wait
                || readiness.busy().is_some()
            {
                continue;
            }

            // 4. Issue health probe
            let sequence = next_probe_id.fetch_add(1, Ordering::Relaxed);
            let id = format!("{health_probe_id_prefix}{sequence}");
            let probe_body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": &id,
                "method": HEALTH_PROBE_METHOD,
                "params": {}
            })
            .to_string();

            let (tx, rx) = tokio::sync::oneshot::channel();
            *health_pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(HealthProbePending {
                id: id.clone(),
                response: tx,
            });

            let deadline = Instant::now() + response_timeout;
            if to_server_tx
                .try_send(PendingServerFrame {
                    body: probe_body,
                    deadline,
                })
                .is_err()
            {
                *health_pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
                continue;
            }

            match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(_)) => {}
                _ => {
                    *health_pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    let became_active =
                        probe_ordinary_epoch.load(Ordering::Acquire) != activity_before_wait;
                    let mut state = probe_state.lock().unwrap_or_else(|e| e.into_inner());
                    if became_active || state.valid_evidence_epoch != evidence_before_wait {
                        state.consecutive_timeouts = 0;
                    } else {
                        state.consecutive_timeouts += 1;
                        tracing::warn!(
                            session_id,
                            consecutive_timeouts = state.consecutive_timeouts,
                            "editor language server health probe timed out"
                        );
                        if state.consecutive_timeouts >= MAX_IDLE_PROBE_TIMEOUTS {
                            tracing::error!(
                                session_id,
                                "editor language server exceeded max idle probe timeouts ({MAX_IDLE_PROBE_TIMEOUTS}); retiring session"
                            );
                            let _ = retire_tx.send(true);
                            break;
                        }
                    }
                }
            }
        }
    })
}
