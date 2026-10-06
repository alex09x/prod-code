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
use std::sync::{Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::{Notify, RwLock, oneshot};
use tokio::time::Instant;

pub(crate) const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const MAX_IDLE_PROBE_TIMEOUTS: usize = 3;
pub(crate) const HEALTH_PROBE_METHOD: &str = "prodCode/healthProbe";
pub(crate) const HEALTH_PROBE_ID_PREFIX: &str = "prod-code-backend-health:";
pub(crate) static NEXT_HEALTH_PROBE_NAMESPACE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
pub(crate) struct ProbeState {
    pub(crate) consecutive_timeouts: usize,
    pub(crate) valid_evidence_epoch: u64,
    pub(crate) latest_valid_sequence: u64,
    pub(crate) valid_completions: u64,
}

pub(crate) struct HealthProbePending {
    pub(crate) id: String,
    pub(crate) response: oneshot::Sender<serde_json::Value>,
}

pub(crate) struct ProbePending {
    pub(crate) id: String,
    pub(crate) pending: Weak<StdMutex<Option<HealthProbePending>>>,
}

impl Drop for ProbePending {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.upgrade() {
            let mut pending = lock_unpoisoned(&pending);
            if pending.as_ref().is_some_and(|slot| slot.id == self.id) {
                pending.take();
            }
        }
    }
}

pub(crate) fn health_probe_sequence(id: &serde_json::Value, id_prefix: &str) -> Option<u64> {
    let suffix = id.as_str()?.strip_prefix(id_prefix)?;
    let sequence = suffix.parse::<u64>().ok()?;
    (sequence != 0 && sequence.to_string() == suffix).then_some(sequence)
}

pub(crate) fn valid_dispatch_response(response: &serde_json::Value) -> bool {
    let Some(envelope) = response.as_object() else {
        return false;
    };
    if envelope.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
        || envelope.get("id").is_none_or(serde_json::Value::is_null)
        || envelope.contains_key("method")
    {
        return false;
    }
    match (envelope.get("result"), envelope.get("error")) {
        (Some(_), None) => true,
        (None, Some(error)) => {
            error
                .get("code")
                .and_then(serde_json::Value::as_i64)
                .is_some()
                && error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
        }
        _ => false,
    }
}

pub(crate) fn record_dispatch_liveness(
    ordinary_epoch: &AtomicU64,
    last_activity: &StdMutex<Instant>,
    probe_state: &StdMutex<ProbeState>,
) {
    ordinary_epoch.fetch_add(1, Ordering::AcqRel);
    *lock_unpoisoned(last_activity) = Instant::now();
    let mut state = lock_unpoisoned(probe_state);
    state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
    state.consecutive_timeouts = 0;
}

pub(crate) async fn retire_health_generation(
    child: &Weak<StdMutex<Child>>,
    is_alive: &Weak<AtomicBool>,
    closed: &Weak<Notify>,
    capabilities: &Weak<RwLock<Option<serde_json::Value>>>,
    health_pending: &Weak<StdMutex<Option<HealthProbePending>>>,
) {
    if let Some(is_alive) = is_alive.upgrade() {
        is_alive.store(false, Ordering::Release);
    }
    if let Some(closed) = closed.upgrade() {
        closed.notify_waiters();
        closed.notify_one();
    }
    if let Some(child) = child.upgrade() {
        let _ = lock_unpoisoned(&child).start_kill();
    }
    if let Some(health_pending) = health_pending.upgrade() {
        lock_unpoisoned(&health_pending).take();
    }
    if let Some(capabilities) = capabilities.upgrade() {
        *capabilities.write().await = None;
    }
}
