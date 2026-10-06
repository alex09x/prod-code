/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::probe::{
    HEALTH_PROBE_METHOD, HealthProbePending, MAX_IDLE_PROBE_TIMEOUTS, ProbePending,
    lock_unpoisoned, retire_health_generation,
};
use super::request_history::OwnedTask;
use super::worker::BackendWorker;
use super::writer::{FrameWrite, FrameWriter};

impl BackendWorker {
    pub(crate) fn start_health_probe(&mut self, interval: Duration, response_timeout: Duration) {
        let stdin = Arc::downgrade(&self.writer.stdin);
        let child = Arc::downgrade(&self._child);
        let is_alive = Arc::downgrade(&self.is_alive);
        let closed = Arc::downgrade(&self.closed);
        let capabilities = Arc::downgrade(&self.capabilities);
        let ordinary_epoch = Arc::downgrade(&self.ordinary_epoch);
        let last_activity = Arc::downgrade(&self.last_activity);
        let probe_state = Arc::downgrade(&self.probe_state);
        let next_probe_id = Arc::downgrade(&self.next_probe_id);
        let health_pending = Arc::downgrade(&self.health_probe_pending);
        let issued_requests = Arc::downgrade(&self.issued_requests);
        let health_probe_id_prefix = Arc::clone(&self.writer.health_probe_id_prefix);
        let engine = Arc::clone(&self.writer.engine);

        self.health_task = Some(OwnedTask(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let Some(alive) = is_alive.upgrade() else {
                    break;
                };
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                let Some(epoch) = ordinary_epoch.upgrade() else {
                    break;
                };
                let Some(activity) = last_activity.upgrade() else {
                    break;
                };
                if lock_unpoisoned(&activity).elapsed() < interval {
                    continue;
                }
                let activity_before_wait = epoch.load(Ordering::Acquire);

                let Some(stdin) = stdin.upgrade() else {
                    break;
                };
                let Ok(stdin_guard) = Arc::clone(&stdin).try_lock_owned() else {
                    continue;
                };
                if !alive.load(Ordering::Acquire)
                    || epoch.load(Ordering::Acquire) != activity_before_wait
                    || lock_unpoisoned(&activity).elapsed() < interval
                {
                    continue;
                }

                let Some(state) = probe_state.upgrade() else {
                    break;
                };
                let evidence_before_wait = lock_unpoisoned(&state).valid_evidence_epoch;
                let Some(ids) = next_probe_id.upgrade() else {
                    break;
                };
                let sequence = ids.fetch_add(1, Ordering::AcqRel);
                let id = format!("{health_probe_id_prefix}{sequence}");
                let Some(pending_slot) = health_pending.upgrade() else {
                    break;
                };
                let (tx, rx) = oneshot::channel();
                *lock_unpoisoned(&pending_slot) = Some(HealthProbePending {
                    id: id.clone(),
                    response: tx,
                });
                let pending = ProbePending {
                    id: id.clone(),
                    pending: health_pending.clone(),
                };
                let Some(closed_notify) = closed.upgrade() else {
                    break;
                };
                let Some(history) = issued_requests.upgrade() else {
                    break;
                };
                let frame_writer = FrameWriter {
                    stdin: Arc::clone(&stdin),
                    child: child.clone(),
                    is_alive: Arc::clone(&alive),
                    closed: closed_notify,
                    engine: Arc::clone(&engine),
                    timeout: response_timeout,
                    ordinary_epoch: Arc::clone(&epoch),
                    last_activity: Arc::clone(&activity),
                    probe_state: Arc::clone(&state),
                    issued_requests: history,
                    health_probe_id_prefix: Arc::clone(&health_probe_id_prefix),
                };
                let payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": &id,
                    "method": HEALTH_PROBE_METHOD,
                    "params": {}
                })
                .to_string();
                let header = format!("Content-Length: {}\r\n\r\n", payload.len());
                let deadline = Instant::now() + response_timeout;
                let mut frame = FrameWrite::new(stdin_guard, frame_writer, response_timeout);
                let write_result = async {
                    frame
                        .write_all_until(deadline, header.as_bytes(), "health header")
                        .await?;
                    frame
                        .write_all_until(deadline, payload.as_bytes(), "health payload")
                        .await?;
                    frame.flush_until(deadline).await
                }
                .await;
                if let Err(error) = write_result {
                    if frame.bytes_written == 0 && !frame.faulted {
                        frame.complete = true;
                        tracing::debug!(%error, "backend health probe write made no progress");
                        continue;
                    }
                    tracing::warn!(%error, "backend health probe frame failed; retiring process");
                    drop(frame);
                    retire_health_generation(
                        &child,
                        &is_alive,
                        &closed,
                        &capabilities,
                        &health_pending,
                    )
                    .await;
                    break;
                }
                frame.complete = true;
                drop(frame);
                drop(stdin);
                drop(pending_slot);

                match tokio::time::timeout_at(deadline, rx).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => break,
                    Err(_) => {
                        drop(pending);
                        let became_active = epoch.load(Ordering::Acquire) != activity_before_wait;
                        let should_retire = {
                            let mut state = lock_unpoisoned(&state);
                            if became_active || state.valid_evidence_epoch != evidence_before_wait {
                                state.consecutive_timeouts = 0;
                                false
                            } else {
                                state.consecutive_timeouts += 1;
                                state.consecutive_timeouts >= MAX_IDLE_PROBE_TIMEOUTS
                            }
                        };
                        if should_retire {
                            retire_health_generation(
                                &child,
                                &is_alive,
                                &closed,
                                &capabilities,
                                &health_pending,
                            )
                            .await;
                            break;
                        }
                    }
                }
            }
        })));
    }
}
