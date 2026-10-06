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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

pub(crate) const CHANNEL_CAPACITY: usize = 1024;
pub(crate) const WRITE_BUDGET: Duration = Duration::from_secs(30);
pub(crate) const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);
pub const DEFAULT_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
pub const DEFAULT_HEALTH_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_IDLE_PROBE_TIMEOUTS: usize = 3;
pub const HEALTH_PROBE_METHOD: &str = "prodCode/healthProbe";
pub const HEALTH_PROBE_ID_PREFIX: &str = "prod-code-editor-health:";

pub(crate) static NEXT_HEALTH_PROBE_NAMESPACE: AtomicU64 = AtomicU64::new(1);

#[derive(Default, Debug, Clone)]
pub struct ProbeState {
    pub consecutive_timeouts: usize,
    pub latest_valid_sequence: u64,
    pub valid_evidence_epoch: u64,
    pub valid_completions: u64,
}

pub(crate) struct HealthProbePending {
    pub(crate) id: String,
    pub(crate) response: tokio::sync::oneshot::Sender<serde_json::Value>,
}

pub fn health_probe_sequence(id: &serde_json::Value, id_prefix: &str) -> Option<u64> {
    id.as_str()?.strip_prefix(id_prefix)?.parse::<u64>().ok()
}

pub fn valid_dispatch_response(value: &serde_json::Value) -> bool {
    let Some(envelope) = value.as_object() else {
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

pub(crate) fn record_liveness(
    ordinary_epoch: &AtomicU64,
    last_activity: &std::sync::Mutex<Instant>,
    probe_state: &std::sync::Mutex<ProbeState>,
) {
    ordinary_epoch.fetch_add(1, Ordering::AcqRel);
    *last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    let mut state = probe_state.lock().unwrap_or_else(|e| e.into_inner());
    state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
    state.consecutive_timeouts = 0;
}

/// Options controlling editor proxy timeouts, budgets, and idle health probing.
#[derive(Debug, Clone)]
pub struct EditorProxyOptions {
    pub write_budget: Duration,
    pub teardown_budget: Duration,
    pub health_probe_interval: Option<Duration>,
    pub health_response_timeout: Duration,
    pub probe_state: Option<Arc<std::sync::Mutex<ProbeState>>>,
}

impl Default for EditorProxyOptions {
    fn default() -> Self {
        Self {
            write_budget: WRITE_BUDGET,
            teardown_budget: TEARDOWN_BUDGET,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
            health_response_timeout: DEFAULT_HEALTH_RESPONSE_TIMEOUT,
            probe_state: None,
        }
    }
}
