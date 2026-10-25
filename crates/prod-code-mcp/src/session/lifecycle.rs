/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::types::LspSession;
use crate::watch::{Lease, RootIdentity, root_identity};

// Active/queued callers own an Arc and are never subject to the idle cache budget.
pub(super) const MAX_IDLE: usize = 16;
const MAX_IDLE_AGE: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PoolKey {
    pub remote: SocketAddr,
    pub root: PathBuf,
    pub identity: Option<RootIdentity>,
    pub subpath: String,
    pub engine: String,
}

pub(crate) struct Slot {
    session: tokio::sync::Mutex<Option<LspSession>>,
    pub generation: AtomicU64,
    last_used: Mutex<Instant>,
    _watch: Lease,
}

impl Deref for Slot {
    type Target = tokio::sync::Mutex<Option<LspSession>>;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

pub(crate) type SessionSlot = Arc<Slot>;

pub(crate) fn pool() -> &'static Mutex<HashMap<PoolKey, SessionSlot>> {
    static POOL: OnceLock<Mutex<HashMap<PoolKey, SessionSlot>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn retire_idle(budget: usize) {
    let keys: Vec<_> = pool()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    let invalid: HashSet<_> = keys
        .into_iter()
        .filter(|key| key.identity.is_none() || root_identity(&key.root) != key.identity)
        .collect();
    let mut sessions = pool().lock().unwrap_or_else(|e| e.into_inner());
    let mut idle: Vec<_> = sessions
        .iter()
        .filter(|(_, slot)| Arc::strong_count(slot) == 1)
        .map(|(key, slot)| {
            (
                key.clone(),
                *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()),
            )
        })
        .collect();
    idle.sort_by_key(|(_, at)| *at);
    let excess = idle.len().saturating_sub(budget);
    let mut retired = Vec::new();
    for (index, (key, at)) in idle.into_iter().enumerate() {
        if index < excess || invalid.contains(&key) || at.elapsed() >= MAX_IDLE_AGE {
            retired.extend(sessions.remove(&key));
        }
    }
    drop(sessions);
    // Closing sockets and joining watcher threads must never hold a global cache lock.
    drop(retired);
}

pub(crate) async fn session_slot(key: &PoolKey) -> SessionSlot {
    // Filesystem inspection and watcher construction may block; neither holds the pool lock.
    let candidate = Arc::new(Slot {
        session: tokio::sync::Mutex::new(None),
        generation: AtomicU64::new(0),
        last_used: Mutex::new(Instant::now()),
        _watch: crate::watch::lease(&key.root),
    });
    let slot = {
        let mut sessions = pool().lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            sessions
                .entry(key.clone())
                .or_insert_with(|| Arc::clone(&candidate)),
        )
    };
    *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    drop(candidate);
    retire_idle(MAX_IDLE.saturating_sub(1));
    slot
}

/// Retire idle owners on completion and on cancellation, even without a subsequent query.
pub(super) struct QueryLease(Option<SessionSlot>);

impl QueryLease {
    pub fn new(slot: SessionSlot) -> Self {
        Self(Some(slot))
    }
}

impl Deref for QueryLease {
    type Target = Slot;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().expect("live query lease")
    }
}

impl Drop for QueryLease {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
            drop(slot);
        }
        retire_idle(MAX_IDLE);
    }
}
