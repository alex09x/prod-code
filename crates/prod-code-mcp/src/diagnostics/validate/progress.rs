/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::{Arc, Mutex};

/// A read-only snapshot for a caller's quiet-progress reporter. Updating a phase
/// never writes output, so MCP callers and fast CLI batches remain silent.
#[derive(Clone)]
pub struct ValidationProgress(Arc<Mutex<(String, usize, usize)>>);

impl ValidationProgress {
    pub fn new(total: usize) -> Self {
        Self(Arc::new(Mutex::new((
            "preparing validation".into(),
            0,
            total,
        ))))
    }

    /// Current phase, files with completed reports, and total requested files.
    pub fn snapshot(&self) -> (String, usize, usize) {
        self.0.lock().expect("validation progress lock").clone()
    }

    pub(super) fn phase(&self, phase: &str) {
        self.0.lock().expect("validation progress lock").0 = phase.into();
    }

    pub(super) fn completed(&self, count: usize) {
        let mut state = self.0.lock().expect("validation progress lock");
        state.1 += count;
    }
}
