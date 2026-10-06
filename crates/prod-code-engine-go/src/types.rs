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
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard, Weak};
use tokio::process::Child;
use tokio::sync::{RwLock, oneshot};

use crate::config::HEALTH_PROBE_ID_PREFIX;
use crate::engine::GoEngine;

pub(crate) struct HealthProbeTask(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for HealthProbeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct OrdinaryActivity {
    active: Arc<AtomicUsize>,
}

impl OrdinaryActivity {
    pub(crate) fn begin(
        active: &Arc<AtomicUsize>,
        epoch: &Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        epoch.fetch_add(1, Ordering::AcqRel);
        Self {
            active: Arc::clone(active),
        }
    }
}

impl Drop for OrdinaryActivity {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
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

pub(crate) struct HealthProbePending {
    pub(crate) id: String,
    pub(crate) response: oneshot::Sender<serde_json::Value>,
}

#[derive(Default)]
pub(crate) struct ProbeState {
    pub(crate) consecutive_timeouts: usize,
    pub(crate) valid_evidence_epoch: u64,
    pub(crate) latest_valid_sequence: u64,
    pub(crate) valid_completions: u64,
}

pub(crate) struct PendingRequest {
    pub(crate) id: u64,
    pub(crate) pending: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub(crate) child: Arc<StdMutex<Child>>,
    pub(crate) is_alive: Arc<AtomicBool>,
    pub(crate) frame_written: bool,
}

impl PendingRequest {
    fn retire_if_partial(&self) {
        if self.frame_written {
            return;
        }
        self.is_alive.store(false, Ordering::Release);
        let _ = lock_unpoisoned(&self.child).start_kill();
        lock_unpoisoned(&self.pending).clear();
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        lock_unpoisoned(&self.pending).remove(&self.id);
        self.retire_if_partial();
    }
}

pub(crate) struct FrameWrite {
    pub(crate) child: Weak<StdMutex<Child>>,
    pub(crate) pending: Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub(crate) is_alive: Weak<AtomicBool>,
    pub(crate) started: bool,
    pub(crate) complete: bool,
}

impl FrameWrite {
    pub(crate) fn retire(&self) {
        if let Some(is_alive) = self.is_alive.upgrade() {
            is_alive.store(false, Ordering::Release);
        }
        if let Some(child) = self.child.upgrade() {
            let _ = lock_unpoisoned(&child).start_kill();
        }
        if let Some(pending) = self.pending.upgrade() {
            lock_unpoisoned(&pending).clear();
        }
    }
}

impl Drop for FrameWrite {
    fn drop(&mut self) {
        if self.started && !self.complete {
            self.retire();
        }
    }
}

pub(crate) fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn validate_initialize_response(
    response: &serde_json::Value,
    server: &str,
) -> Result<serde_json::Value> {
    let envelope = response
        .as_object()
        .context("initialize response must be a JSON object")?;
    if envelope.contains_key("error") {
        anyhow::bail!("{server} refused initialization: {}", envelope["error"]);
    }
    let result = envelope
        .get("result")
        .filter(|result| result.is_object())
        .context("initialize response has no successful result object")?;
    result
        .get("capabilities")
        .filter(|capabilities| capabilities.is_object())
        .cloned()
        .context("initialize response has no capabilities object")
}

pub(crate) fn health_probe_sequence(
    response: &serde_json::Value,
    next_probe_id: u64,
) -> Option<u64> {
    let suffix = response
        .get("id")?
        .as_str()?
        .strip_prefix(HEALTH_PROBE_ID_PREFIX)?;
    let sequence = suffix.parse::<u64>().ok()?;
    (sequence != 0 && sequence < next_probe_id && sequence.to_string() == suffix)
        .then_some(sequence)
}

pub(crate) fn valid_health_response(response: &serde_json::Value, id: &str) -> bool {
    let Some(envelope) = response.as_object() else {
        return false;
    };
    if envelope.get("jsonrpc").and_then(|value| value.as_str()) != Some("2.0")
        || envelope.get("id").and_then(|value| value.as_str()) != Some(id)
        || envelope.contains_key("method")
    {
        return false;
    }
    match (envelope.get("result"), envelope.get("error")) {
        (Some(_), None) => true,
        (None, Some(error)) => {
            error.get("code").and_then(|value| value.as_i64()).is_some()
                && error
                    .get("message")
                    .and_then(|value| value.as_str())
                    .is_some()
        }
        _ => false,
    }
}

pub(crate) async fn retire_health_generation(
    child: &Weak<StdMutex<Child>>,
    pending: &Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    health_pending: &Weak<StdMutex<Option<HealthProbePending>>>,
    is_alive: &Weak<AtomicBool>,
    capabilities: &Weak<RwLock<Option<serde_json::Value>>>,
) {
    if let Some(is_alive) = is_alive.upgrade() {
        is_alive.store(false, Ordering::Release);
    }
    if let Some(child) = child.upgrade() {
        let _ = lock_unpoisoned(&child).start_kill();
    }
    if let Some(pending) = pending.upgrade() {
        lock_unpoisoned(&pending).clear();
    }
    if let Some(health_pending) = health_pending.upgrade() {
        lock_unpoisoned(&health_pending).take();
    }
    if let Some(capabilities) = capabilities.upgrade() {
        *capabilities.write().await = None;
    }
}

pub(crate) struct InitializationGuard<'a> {
    pub(crate) engine: &'a GoEngine,
    pub(crate) complete: bool,
}

impl Drop for InitializationGuard<'_> {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        self.engine.retire_generation();
        if let Ok(mut capabilities) = self.engine.capabilities.try_write() {
            *capabilities = None;
        } else {
            let capabilities = Arc::clone(&self.engine.capabilities);
            drop(tokio::spawn(async move {
                *capabilities.write().await = None;
            }));
        }
    }
}
