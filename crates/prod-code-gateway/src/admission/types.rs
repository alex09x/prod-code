/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Data types, shortfall reporting, and memory measurement primitives.

use prod_code_protocol::MEMORY_PRESSURE_USED;
use std::sync::Arc;
use std::time::Duration;

use super::Admission;

pub(crate) const MIB: u64 = 1 << 20;
pub(crate) const GIB: u64 = 1 << 30;

/// How long a reservation outlives its load. A language server keeps growing while it indexes
/// after it answers `initialize`, and the host's figure trails what a load allocated; until it
/// settles the reservation is counted as well, which over-counts rather than under-counts.
pub const LOAD_SETTLE: Duration = Duration::from_secs(120);

/// The host's memory, `(available, total)` in bytes; `None` when it cannot be read.
pub type MemoryProbe = Arc<dyn Fn() -> Option<(u64, u64)> + Send + Sync>;

/// What a new engine of `engine` is assumed to need until it has loaded and settled. An
/// in-process rust-analyzer database for a large workspace takes several GiB; the language
/// servers run as their own processes and are smaller.
pub fn default_reserve(engine: &str) -> u64 {
    match engine {
        "rust" => 4 * GIB,
        "cpp" | "swift" | "typescript" | "go" | "python" => GIB,
        _ => 256 * MIB,
    }
}

#[derive(Default)]
pub(crate) struct Ledger {
    /// Wider than any reservation, so that however large a configured one is, returning it
    /// takes back exactly what it added.
    pub(crate) reserved: u128,
    pub(crate) loads: usize,
}

impl Ledger {
    pub(crate) fn reserved_bytes(&self) -> u64 {
        u64::try_from(self.reserved).unwrap_or(u64::MAX)
    }
}

/// Memory held for one admitted load; returned when dropped.
#[must_use = "a reservation is returned as soon as it is dropped"]
pub struct Reservation {
    pub(crate) admission: Arc<Admission>,
    pub(crate) bytes: u64,
}

impl Reservation {
    /// Keeps the reservation for the settle time after its load, then returns it, on a timer
    /// rather than by polling. Needs a Tokio runtime.
    pub fn release_after_settling(self) {
        let settle = self.admission.settle;
        if settle.is_zero() {
            return;
        }
        tokio::spawn(async move {
            tokio::time::sleep(settle).await;
            drop(self);
        });
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut ledger = self.admission.ledger();
        ledger.reserved = ledger.reserved.saturating_sub(u128::from(self.bytes));
        ledger.loads = ledger.loads.saturating_sub(1);
    }
}

/// Why a new engine was not admitted: the figures the decision was made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortfall {
    pub engine: String,
    pub available: u64,
    pub total: u64,
    /// Held for loads admitted earlier and not yet settled.
    pub reserved: u64,
    pub loads: usize,
    /// What this engine would reserve.
    pub needed: u64,
}

impl Shortfall {
    /// Bytes that would have to be freed for the engine to fit under the limit.
    pub fn excess(&self) -> u64 {
        let committed = self
            .total
            .saturating_sub(self.available)
            .saturating_add(self.reserved)
            .saturating_add(self.needed);
        committed.saturating_sub(limit_bytes(self.total))
    }
}

/// A new engine refused for lack of memory, with what the client can do about it.
#[derive(Debug, Clone)]
pub struct CapacityRefused {
    pub shortfall: Shortfall,
    /// Idle engines unloaded while trying to make room.
    pub reclaimed: usize,
}

impl std::fmt::Display for CapacityRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = &self.shortfall;
        let used = s.total.saturating_sub(s.available);
        write!(
            f,
            "capacity: this node has no memory for a new {} engine (memory {}% used, {} GiB held \
             for {} load(s) in flight, a new engine counts {} GiB, the limit is {}% of {} GiB",
            s.engine,
            used.saturating_mul(100) / s.total.max(1),
            gib(s.reserved),
            s.loads,
            gib(s.needed),
            (MEMORY_PRESSURE_USED * 100.0).round() as u32,
            gib(s.total),
        )?;
        if self.reclaimed > 0 {
            write!(f, "; {} idle engine(s) were unloaded", self.reclaimed)?;
        }
        write!(
            f,
            "). Engines already loaded here keep answering. Retry in a few minutes, once loads \
             in flight settle or idle engines are unloaded, or place this workspace on another \
             node (`prod-code cluster` shows which have room)."
        )
    }
}

impl std::error::Error for CapacityRefused {}

pub(crate) fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB as f64)
}

pub(crate) fn limit_bytes(total: u64) -> u64 {
    (total as f64 * MEMORY_PRESSURE_USED).round() as u64
}
