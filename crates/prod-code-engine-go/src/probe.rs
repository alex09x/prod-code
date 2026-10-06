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
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::config::{HEALTH_PROBE_ID_PREFIX, HEALTH_PROBE_METHOD, MAX_IDLE_PROBE_TIMEOUTS};
use crate::engine::GoEngine;
use crate::types::{
    FrameWrite, HealthProbePending, HealthProbeTask, ProbePending, lock_unpoisoned,
    retire_health_generation, valid_health_response,
};

impl GoEngine {
    pub(crate) fn start_health_probe(&mut self, interval: Duration) {
        let stdin = Arc::downgrade(&self.stdin);
        let pending = Arc::downgrade(&self.pending_requests);
        let next_probe_id = Arc::downgrade(&self.next_probe_id);
        let health_pending = Arc::downgrade(&self.health_probe_pending);
        let child = Arc::downgrade(&self._child);
        let is_alive = Arc::downgrade(&self.is_alive);
        let capabilities = Arc::downgrade(&self.capabilities);
        let readiness = Arc::downgrade(&self.readiness);
        let ordinary_activity = Arc::downgrade(&self.ordinary_activity);
        let ordinary_epoch = Arc::downgrade(&self.ordinary_epoch);
        let probe_state = Arc::downgrade(&self.probe_state);
        let response_timeout = self.request_timeout;

        self._health_probe = Some(HealthProbeTask(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let Some(alive) = is_alive.upgrade() else {
                    break;
                };
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                let Some(activity) = ordinary_activity.upgrade() else {
                    break;
                };
                let Some(ready) = readiness.upgrade() else {
                    break;
                };
                let Some(pending_requests) = pending.upgrade() else {
                    break;
                };
                if activity.load(Ordering::Acquire) != 0
                    || ready.busy().is_some()
                    || !lock_unpoisoned(&pending_requests).is_empty()
                {
                    continue;
                }

                let Some(stdin) = stdin.upgrade() else {
                    break;
                };
                let Ok(mut writer) = stdin.try_lock() else {
                    continue;
                };
                // An ordinary operation may have started while the probe acquired stdin.
                if !alive.load(Ordering::Acquire)
                    || activity.load(Ordering::Acquire) != 0
                    || ready.busy().is_some()
                    || !lock_unpoisoned(&pending_requests).is_empty()
                {
                    continue;
                }

                let Some(health_pending_slot) = health_pending.upgrade() else {
                    break;
                };
                let Some(ids_allocator) = next_probe_id.upgrade() else {
                    break;
                };
                let Some(epoch) = ordinary_epoch.upgrade() else {
                    break;
                };
                let activity_before_wait = epoch.load(Ordering::Acquire);
                let Some(state) = probe_state.upgrade() else {
                    break;
                };
                let evidence_before_wait = lock_unpoisoned(&state).valid_evidence_epoch;
                let sequence = ids_allocator.fetch_add(1, Ordering::AcqRel);
                let id = format!("{HEALTH_PROBE_ID_PREFIX}{sequence}");
                let payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": &id,
                    "method": HEALTH_PROBE_METHOD,
                    "params": {}
                });
                let body = payload.to_string();
                let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
                let (tx, rx) = oneshot::channel();
                *lock_unpoisoned(&health_pending_slot) = Some(HealthProbePending {
                    id: id.clone(),
                    response: tx,
                });
                let probe_pending = ProbePending {
                    id: id.clone(),
                    pending: health_pending.clone(),
                };
                let mut frame_write = FrameWrite {
                    child: child.clone(),
                    pending: pending.clone(),
                    is_alive: is_alive.clone(),
                    started: true,
                    complete: false,
                };
                let deadline = tokio::time::Instant::now() + response_timeout;
                let wrote =
                    match tokio::time::timeout_at(deadline, writer.write_all(frame.as_bytes()))
                        .await
                    {
                        Ok(Ok(())) => matches!(
                            tokio::time::timeout_at(deadline, writer.flush()).await,
                            Ok(Ok(()))
                        ),
                        _ => false,
                    };
                if !wrote {
                    // A write may have put only a prefix on the stream. Retire before the
                    // writer unlocks so no ordinary frame can follow that prefix.
                    frame_write.retire();
                    frame_write.complete = true;
                    drop(writer);
                    retire_health_generation(
                        &child,
                        &pending,
                        &health_pending,
                        &is_alive,
                        &capabilities,
                    )
                    .await;
                    break;
                }
                frame_write.complete = true;
                drop(frame_write);
                drop(writer);
                drop(stdin);
                drop(health_pending_slot);
                drop(pending_requests);

                match tokio::time::timeout_at(deadline, rx).await {
                    Ok(Ok(response)) if valid_health_response(&response, &id) => {}
                    Ok(Ok(_)) => {
                        retire_health_generation(
                            &child,
                            &pending,
                            &health_pending,
                            &is_alive,
                            &capabilities,
                        )
                        .await;
                        break;
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {
                        drop(probe_pending);
                        let became_active = activity.load(Ordering::Acquire) != 0
                            || epoch.load(Ordering::Acquire) != activity_before_wait
                            || ready.busy().is_some();
                        if became_active {
                            continue;
                        }
                        let should_retire = {
                            let mut state = lock_unpoisoned(&state);
                            if state.valid_evidence_epoch != evidence_before_wait {
                                false
                            } else {
                                state.consecutive_timeouts += 1;
                                state.consecutive_timeouts >= MAX_IDLE_PROBE_TIMEOUTS
                            }
                        };
                        if should_retire {
                            retire_health_generation(
                                &child,
                                &pending,
                                &health_pending,
                                &is_alive,
                                &capabilities,
                            )
                            .await;
                            break;
                        }
                        continue;
                    }
                }
            }
        })));
    }
}
