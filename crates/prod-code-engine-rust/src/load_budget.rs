/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Bound cold analysis loads across all workspaces, including validation engines.

use std::sync::{Condvar, Mutex, OnceLock};

pub(crate) struct LoadBudget {
    pub(crate) workers: usize,
    available: Mutex<usize>,
    ready: Condvar,
}

impl LoadBudget {
    fn new(cpus: usize) -> Self {
        // Leave room for interactive queries and already-running commands. Cargo's default
        // is every CPU *per load*, which oversubscribes a fleet of fresh worktrees (#408).
        let budget = (cpus / 2).max(1);
        let workers = budget.min(8);
        Self {
            workers,
            available: Mutex::new(budget / workers),
            ready: Condvar::new(),
        }
    }

    pub(crate) fn acquire(&self) -> LoadPermit<'_> {
        let mut available = self.available.lock().unwrap_or_else(|e| e.into_inner());
        while *available == 0 {
            available = self
                .ready
                .wait(available)
                .unwrap_or_else(|e| e.into_inner());
        }
        *available -= 1;
        LoadPermit { budget: self }
    }
}

pub(crate) struct LoadPermit<'a> {
    budget: &'a LoadBudget,
}

impl Drop for LoadPermit<'_> {
    fn drop(&mut self) {
        let mut available = self
            .budget
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *available += 1;
        self.budget.ready.notify_one();
    }
}

pub(crate) fn shared() -> &'static LoadBudget {
    static BUDGET: OnceLock<LoadBudget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        LoadBudget::new(std::thread::available_parallelism().map_or(1, |n| n.get()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn simultaneous_loads_wait_until_a_slot_is_released() {
        let budget = Arc::new(LoadBudget::new(32));
        let first = budget.acquire();
        let second = budget.acquire();
        let (entered, received) = mpsc::channel();
        let other = Arc::clone(&budget);
        let worker = std::thread::spawn(move || {
            let _permit = other.acquire();
            entered.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
        drop(second);
        assert_eq!(*budget.available.lock().unwrap(), 2);
    }

    #[test]
    fn a_failed_load_returns_its_slot_even_during_unwinding() {
        let budget = LoadBudget::new(1);
        let result = std::panic::catch_unwind(|| {
            let _permit = budget.acquire();
            panic!("load failed");
        });
        assert!(result.is_err());
        assert_eq!(*budget.available.lock().unwrap(), 1);
        let _again = budget.acquire();
    }

    #[test]
    fn load_capacity_is_nonzero_and_leaves_room_for_queries() {
        for cpus in [0, 1, 2, 3, 4, 8, 16, 32, 128, 256] {
            let budget = LoadBudget::new(cpus);
            let slots = *budget.available.lock().unwrap();
            assert!((1..=8).contains(&budget.workers));
            assert!(slots > 0);
            assert!(slots * budget.workers <= (cpus / 2).max(1));
        }
        assert!(std::ptr::eq(shared(), shared()));
    }
}
