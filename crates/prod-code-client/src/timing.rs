/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::env;

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
/// Phase timings for one part of an invocation, printed to stderr when `PROD_CODE_TIMING=1`.
/// `label` says which part, so the work before a query is reported separately from the query.
pub struct QueryTiming {
    enabled: bool,
    label: &'static str,
    start: std::time::Instant,
    last: std::time::Instant,
    phases: Vec<(&'static str, f64)>,
}

impl QueryTiming {
    pub fn new() -> Self {
        Self::labelled("query")
    }

    pub fn labelled(label: &'static str) -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: env::var_os("PROD_CODE_TIMING").is_some(),
            label,
            start: now,
            last: now,
            phases: Vec::new(),
        }
    }

    pub fn mark(&mut self, phase: &'static str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        self.phases
            .push((phase, (now - self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }

    pub fn report(&self) {
        if !self.enabled {
            return;
        }
        let total = self.start.elapsed().as_secs_f64() * 1000.0;
        let parts: Vec<String> = self
            .phases
            .iter()
            .map(|(name, ms)| format!("{name}={ms:.1}ms"))
            .collect();
        let label = self.label;
        eprintln!("[timing] {label} total={total:.1}ms {}", parts.join(" "));
    }
}
