//! Memory-aware admission of new engines (#433).
//!
//! Placement keeps new workspaces off a node that is already short of memory, and idle engines
//! are unloaded after a while, but neither stops a node from taking more engines at once than it
//! can hold: a dozen fresh worktrees handshaking together each saw the memory as it was before
//! any of them had loaded. Every new engine therefore reserves what it is assumed to need before
//! it loads, and is refused when the host's used memory plus the reservations of loads still in
//! flight plus its own would pass [`MEMORY_PRESSURE_USED`]. Engines already loaded are never
//! affected: their sessions attach without asking.

use prod_code_protocol::MEMORY_PRESSURE_USED;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

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

/// Admits new engines against the host's memory and the loads already admitted.
pub struct Admission {
    probe: MemoryProbe,
    /// `--engine-reserve-mib`: one reservation for every engine instead of [`default_reserve`].
    reserve_override: Option<u64>,
    /// How long a reservation is held after its load; see [`LOAD_SETTLE`].
    settle: Duration,
    ledger: Mutex<Ledger>,
}

#[derive(Default)]
struct Ledger {
    /// Wider than any reservation, so that however large a configured one is, returning it
    /// takes back exactly what it added.
    reserved: u128,
    loads: usize,
}

impl Ledger {
    fn reserved_bytes(&self) -> u64 {
        u64::try_from(self.reserved).unwrap_or(u64::MAX)
    }
}

/// Memory held for one admitted load; returned when dropped.
#[must_use = "a reservation is returned as soon as it is dropped"]
pub struct Reservation {
    admission: Arc<Admission>,
    bytes: u64,
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

fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB as f64)
}

fn limit_bytes(total: u64) -> u64 {
    (total as f64 * MEMORY_PRESSURE_USED).round() as u64
}

impl Admission {
    /// Admission against this host's memory; `reserve_mib` of 0 keeps the per-engine defaults.
    pub fn host(reserve_mib: u64) -> Self {
        Self::with_probe(Arc::new(crate::memory::system_memory), reserve_mib, LOAD_SETTLE)
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

    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
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

/// A probe that reports `snapshots` in turn and then the last one again, for tests.
#[cfg(test)]
pub fn scripted_probe(snapshots: Vec<(u64, u64)>) -> MemoryProbe {
    let queue = Mutex::new(std::collections::VecDeque::from(snapshots));
    Arc::new(move || {
        let mut queue = queue.lock().unwrap();
        if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().copied()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    /// A host with `used` of `total` GiB in use.
    fn host(used: u64, total: u64) -> (u64, u64) {
        ((total - used) * GIB, total * GIB)
    }

    /// With 80 of 100 GiB in use there are 5 GiB under the 85% limit: two engines of 2 GiB
    /// fit, a third does not, and a returned reservation makes room again.
    #[test]
    fn reservations_of_loads_in_flight_count_against_the_limit() {
        let admission = Arc::new(Admission::with_probe(
            scripted_probe(vec![host(80, 100)]),
            2048,
            LOAD_SETTLE,
        ));
        let first = admission.try_reserve("rust").expect("first fits");
        let _second = admission.try_reserve("go").expect("second fits");
        assert_eq!(admission.reserved_bytes(), 4 * GIB);
        let refused = admission.try_reserve("rust").expect_err("third passes the limit");
        assert_eq!(refused.reserved, 4 * GIB);
        assert_eq!(refused.loads, 2);
        assert_eq!(refused.excess(), GIB);
        drop(first);
        assert_eq!(admission.reserved_bytes(), 2 * GIB);
        let _third = admission.try_reserve("rust").expect("fits once one is returned");
    }

    /// Admissions racing on one snapshot never share its headroom: of eight simultaneous
    /// requests for 2 GiB with 5 GiB free under the limit, exactly two are admitted.
    #[test]
    fn simultaneous_admissions_never_overcommit() {
        let admission = Arc::new(Admission::with_probe(
            scripted_probe(vec![host(80, 100)]),
            2048,
            LOAD_SETTLE,
        ));
        let start = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let admission = Arc::clone(&admission);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    admission.try_reserve("rust").ok()
                })
            })
            .collect();
        let admitted: Vec<Reservation> = workers
            .into_iter()
            .filter_map(|w| w.join().unwrap())
            .collect();
        assert_eq!(admitted.len(), 2);
        assert_eq!(admission.reserved_bytes(), 4 * GIB);
        drop(admitted);
        assert_eq!(admission.reserved_bytes(), 0);
        assert_eq!(admission.ledger().loads, 0);
    }

    /// A host already past the limit takes nothing; one whose memory is unknown takes all.
    #[test]
    fn a_host_past_the_limit_admits_nothing_and_an_unknown_one_everything() {
        let full = Arc::new(Admission::with_probe(
            scripted_probe(vec![host(86, 100)]),
            1,
            LOAD_SETTLE,
        ));
        assert!(full.try_reserve("text").is_err());
        let unknown = Arc::new(Admission::unbounded());
        let held: Vec<_> = (0..64)
            .map(|_| unknown.try_reserve("rust").expect("unknown host admits"))
            .collect();
        assert_eq!(unknown.reserved_bytes(), 64 * default_reserve("rust"));
        drop(held);
    }

    #[test]
    fn engines_reserve_by_kind_unless_one_size_is_configured() {
        let defaults = Admission::host(0);
        assert_eq!(defaults.reserve_for("rust"), 4 * GIB);
        assert_eq!(defaults.reserve_for("cpp"), GIB);
        assert_eq!(defaults.reserve_for("text"), 256 * MIB);
        assert_eq!(Admission::host(512).reserve_for("rust"), 512 * MIB);
    }

    /// A configured reservation past any host saturates instead of overflowing: a known host
    /// refuses every new engine with the usual message, an unknown one still admits, and
    /// returning one of two such reservations leaves the other counted in full.
    #[test]
    fn a_reservation_past_any_host_saturates_and_refuses() {
        let full = Arc::new(Admission::with_probe(
            scripted_probe(vec![host(10, 100)]),
            u64::MAX,
            LOAD_SETTLE,
        ));
        assert_eq!(full.reserve_for("rust"), u64::MAX);
        let shortfall = full.try_reserve("rust").expect_err("nothing holds u64::MAX bytes");
        assert!(shortfall.excess() > 0);
        let text = CapacityRefused {
            shortfall,
            reclaimed: 0,
        }
        .to_string();
        assert!(text.starts_with("capacity: "), "{text}");
        assert_eq!(full.reserved_bytes(), 0);

        let unknown = Arc::new(Admission::with_probe(Arc::new(|| None), u64::MAX, LOAD_SETTLE));
        let mut held: Vec<_> = (0..2)
            .map(|_| unknown.try_reserve("rust").expect("unknown host admits"))
            .collect();
        assert_eq!(unknown.reserved_bytes(), u64::MAX);
        drop(held.pop());
        assert_eq!(unknown.reserved_bytes(), u64::MAX, "the other is still held");
        assert_eq!(unknown.ledger().loads, 1);
        drop(held);
        assert_eq!(unknown.reserved_bytes(), 0);
    }

    /// The refusal names capacity as the cause, the figures, and both ways forward.
    #[test]
    fn a_refusal_says_capacity_and_how_to_go_on() {
        let refused = CapacityRefused {
            shortfall: Shortfall {
                engine: "rust".to_string(),
                available: 16 * GIB,
                total: 100 * GIB,
                reserved: 4 * GIB,
                loads: 1,
                needed: 4 * GIB,
            },
            reclaimed: 2,
        };
        let text = refused.to_string();
        for part in [
            "capacity: this node has no memory for a new rust engine",
            "memory 84% used",
            "4.0 GiB held for 1 load(s) in flight",
            "the limit is 85% of 100.0 GiB",
            "2 idle engine(s) were unloaded",
            "keep answering",
            "Retry in a few minutes",
            "another node",
        ] {
            assert!(text.contains(part), "{part:?} missing from {text}");
        }
    }

    /// A reservation outlives its load by the settle time, and is then returned by a timer,
    /// not by a poll.
    #[tokio::test]
    async fn a_reservation_is_returned_after_its_load_settles() {
        let admission = Arc::new(Admission::with_probe(
            scripted_probe(vec![host(10, 100)]),
            1024,
            Duration::from_millis(100),
        ));
        admission
            .try_reserve("rust")
            .unwrap()
            .release_after_settling();
        assert_eq!(admission.reserved_bytes(), GIB, "held while the load settles");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(admission.reserved_bytes(), 0);
        admission.try_reserve("rust").unwrap().release_after_settling();
        let unbounded = Arc::new(Admission::unbounded());
        unbounded.try_reserve("rust").unwrap().release_after_settling();
        assert_eq!(unbounded.reserved_bytes(), 0, "no settle time, returned at once");
    }
}
