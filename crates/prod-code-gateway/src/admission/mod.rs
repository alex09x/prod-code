/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Memory-aware admission of new engines (#433).
//!
//! Placement keeps new workspaces off a node that is already short of memory, and idle engines
//! are unloaded after a while, but neither stops a node from taking more engines at once than it
//! can hold: a dozen fresh worktrees handshaking together each saw the memory as it was before
//! any of them had loaded. Every new engine therefore reserves what it is assumed to need before
//! it loads, and is refused when the host's used memory plus the reservations of loads still in
//! flight plus its own would pass [`MEMORY_PRESSURE_USED`](prod_code_protocol::MEMORY_PRESSURE_USED).
//! Engines already loaded are never affected: their sessions attach without asking.

use std::sync::{Arc, Mutex};
use std::time::Duration;

pub mod types;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub use tests::scripted_probe;

pub use types::{
    CapacityRefused, LOAD_SETTLE, MemoryProbe, Reservation, Shortfall, default_reserve,
};
use types::{Ledger, MIB};

/// Admits new engines against the host's memory and the loads already admitted.
pub struct Admission {
    pub(crate) probe: MemoryProbe,
    /// `--engine-reserve-mib`: one reservation for every engine instead of [`default_reserve`].
    pub(crate) reserve_override: Option<u64>,
    /// How long a reservation is held after its load; see [`LOAD_SETTLE`].
    pub(crate) settle: Duration,
    pub(crate) ledger: Mutex<Ledger>,
}

impl Admission {
    /// Admission against this host's memory; `reserve_mib` of 0 keeps the per-engine defaults.
    pub fn host(reserve_mib: u64) -> Self {
        Self::with_probe(
            Arc::new(crate::memory::system_memory),
            reserve_mib,
            LOAD_SETTLE,
        )
    }

    /// Admission against the memory `probe` reports.
    pub fn with_probe(probe: MemoryProbe, reserve_mib: u64, settle: Duration) -> Self {
        Self {
            probe,
            // A reservation past what any host has refuses every new engine, and says so.
            reserve_override: (reserve_mib > 0).then(|| reserve_mib.saturating_mul(MIB)),
            settle,
            ledger: Mutex::new(Ledger::default()),
        }
    }

    /// Admission that knows nothing of the host, and so admits every engine.
    pub fn unbounded() -> Self {
        Self::with_probe(Arc::new(|| None), 0, Duration::ZERO)
    }

    /// What a new engine of `engine` reserves.
    pub fn reserve_for(&self, engine: &str) -> u64 {
        self.reserve_override
            .unwrap_or_else(|| default_reserve(engine))
    }

    /// Bytes held for loads admitted and not yet settled.
    pub fn reserved_bytes(&self) -> u64 {
        self.ledger().reserved_bytes()
    }

    pub(crate) fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reserves memory for a new engine of `engine`, or says why there is none.
    ///
    /// The host is read under the ledger's lock, so a load whose reservation has been returned
    /// is already in the figure, and two admissions never both count the same headroom. The
    /// lock is held only for that read, never across a load. A host whose memory cannot be
    /// read admits every engine, as placement does.
    pub fn try_reserve(self: &Arc<Self>, engine: &str) -> Result<Reservation, Shortfall> {
        let needed = self.reserve_for(engine);
        let mut ledger = self.ledger();
        if let Some((available, total)) = (self.probe)().filter(|(_, total)| *total > 0) {
            let shortfall = Shortfall {
                engine: engine.to_string(),
                available: available.min(total),
                total,
                reserved: ledger.reserved_bytes(),
                loads: ledger.loads,
                needed,
            };
            if shortfall.excess() > 0 {
                return Err(shortfall);
            }
        }
        ledger.reserved = ledger.reserved.saturating_add(u128::from(needed));
        ledger.loads += 1;
        Ok(Reservation {
            admission: Arc::clone(self),
            bytes: needed,
        })
    }
}
