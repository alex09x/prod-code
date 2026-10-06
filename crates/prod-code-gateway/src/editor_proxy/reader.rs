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
use prod_code_protocol::{
    PathTranslator, WireMessage, readiness::Readiness, transport::read_lsp_frame,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use super::probe::{
    HealthProbePending, ProbeState, health_probe_sequence, record_liveness, valid_dispatch_response,
};
use super::registry::PendingEditorMessage;

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_stdout_reader_task(
    stdout: tokio::process::ChildStdout,
    translator: PathTranslator,
    tx: rapidfire::mpsc::Sender<PendingEditorMessage>,
    health_probe_id_prefix: Arc<str>,
    health_pending: Arc<std::sync::Mutex<Option<HealthProbePending>>>,
    probe_state: Arc<std::sync::Mutex<ProbeState>>,
    last_activity: Arc<std::sync::Mutex<Instant>>,
    ordinary_epoch: Arc<AtomicU64>,
    lsp_initialized: Arc<AtomicBool>,
    initialize_request_id: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    readiness: Arc<Readiness>,
    in_flight_requests: Arc<AtomicUsize>,
    write_budget: Duration,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        while let Some(body) = read_lsp_frame(&mut stdout)
            .await
            .context("reading an editor server output frame")?
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                readiness.on_message(&val);
                let id = val.get("id");
                let method = val.get("method").and_then(|m| m.as_str());
                if method.is_none()
                    && id.is_some()
                    && initialize_request_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref()
                        == id
                {
                    lsp_initialized.store(true, Ordering::Release);
                }

                let probe_seq =
                    id.and_then(|id| health_probe_sequence(id, &health_probe_id_prefix));
                if method.is_none()
                    && let (Some(id_str), Some(seq)) =
                        (id.and_then(serde_json::Value::as_str), probe_seq)
                {
                    if valid_dispatch_response(&val) {
                        let waiter = {
                            let mut pending =
                                health_pending.lock().unwrap_or_else(|e| e.into_inner());
                            if pending.as_ref().is_some_and(|p| p.id == id_str) {
                                pending.take().map(|p| p.response)
                            } else {
                                None
                            }
                        };
                        let mut state = probe_state.lock().unwrap_or_else(|e| e.into_inner());
                        if seq > state.latest_valid_sequence {
                            state.latest_valid_sequence = seq;
                            state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
                            state.consecutive_timeouts = 0;
                            state.valid_completions += 1;
                        }
                        drop(state);
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(val);
                        }
                    }
                    // Withhold private health probe response from the editor!
                    continue;
                }

                if method.is_none() && id.is_some() {
                    in_flight_requests
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                            Some(count.saturating_sub(1))
                        })
                        .ok();
                }
                record_liveness(&ordinary_epoch, &last_activity, &probe_state);
            }

            let editor = translator.translate_lsp_to_client(&body);
            let deadline = Instant::now() + write_budget;
            timeout_at(
                deadline,
                tx.send(PendingEditorMessage {
                    message: WireMessage::LspPayload(editor),
                    deadline,
                }),
            )
            .await
            .context("editor output queue exceeded its deadline")?
            .map_err(|_| anyhow::anyhow!("editor output queue closed"))?;
        }
        Ok(())
    })
}
