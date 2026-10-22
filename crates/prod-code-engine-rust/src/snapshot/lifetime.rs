/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Revocable Salsa handles. An idle caller must never pin a database revision.

use ra_ap_ide::Analysis;
use std::sync::{
    Arc, Mutex, MutexGuard, Weak,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(crate) struct SnapshotRegistry(Mutex<Vec<Weak<AnalysisState>>>);

impl SnapshotRegistry {
    pub(crate) fn track(&self, analysis: Analysis) -> SnapshotAnalysis {
        let state = Arc::new(AnalysisState {
            analysis: Mutex::new(Some(analysis)),
            cancelled: AtomicBool::new(false),
        });
        let mut snapshots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        snapshots.retain(|s| s.strong_count() > 0);
        snapshots.push(Arc::downgrade(&state));
        SnapshotAnalysis(state)
    }

    /// Revoke idle handles before entering Salsa's cancellation wait, WITHOUT the VFS lock.
    /// Active calls release their handles as they unwind from Salsa cancellation.
    pub(crate) fn cancel(&self) -> bool {
        let mut snapshots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut any = false;
        snapshots.retain(|weak| {
            if let Some(state) = weak.upgrade() {
                state.cancel();
                any = true;
                true
            } else {
                false
            }
        });
        any
    }
}

struct AnalysisState {
    analysis: Mutex<Option<Analysis>>,
    cancelled: AtomicBool,
}

impl AnalysisState {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        match self.analysis.try_lock() {
            Ok(mut analysis) => {
                analysis.take();
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                error.into_inner().take();
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
    }
}

/// A query deadline can revoke the Salsa handle without taking the engine or VFS lock.
#[derive(Clone)]
pub struct SnapshotCancellation(Weak<AnalysisState>);

impl SnapshotCancellation {
    pub fn cancel(&self) {
        if let Some(state) = self.0.upgrade() {
            state.cancel();
        }
    }
}

pub(crate) struct SnapshotAnalysis(Arc<AnalysisState>);

impl SnapshotAnalysis {
    pub(crate) fn cancellation_handle(&self) -> SnapshotCancellation {
        SnapshotCancellation(Arc::downgrade(&self.0))
    }

    pub(crate) fn lock(&self) -> anyhow::Result<AnalysisGuard<'_>> {
        let mut guard = self.0.analysis.lock().unwrap_or_else(|e| e.into_inner());
        if self.0.cancelled.load(Ordering::Acquire) || guard.is_none() {
            guard.take();
            anyhow::bail!("Salsa snapshot cancelled");
        }
        Ok(AnalysisGuard {
            state: &self.0,
            guard,
        })
    }
}

pub(crate) struct AnalysisGuard<'a> {
    state: &'a AnalysisState,
    guard: MutexGuard<'a, Option<Analysis>>,
}

impl std::ops::Deref for AnalysisGuard<'_> {
    type Target = Analysis;
    fn deref(&self) -> &Analysis {
        self.guard.as_ref().expect("live analysis guard")
    }
}

impl Drop for AnalysisGuard<'_> {
    fn drop(&mut self) {
        if self.state.cancelled.load(Ordering::Acquire) {
            self.guard.take();
        }
    }
}
