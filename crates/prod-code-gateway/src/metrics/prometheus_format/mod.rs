/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Prometheus text exposition format (version 0.0.4) formatter.

pub mod operation;
pub mod snapshot;

pub use operation::format_operation_metrics;
pub use snapshot::{format_inventory_metrics, format_snapshot_metrics};

use super::Metrics;

/// Escapes a label value according to the Prometheus exposition format specification.
pub fn escape_label_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Formats the current state of [`Metrics`] into a Prometheus exposition text string.
pub fn format_prometheus_metrics(metrics: &Metrics, node_override: Option<&str>) -> String {
    let node_str = node_override
        .map(String::from)
        .unwrap_or_else(|| metrics.node());
    let node = escape_label_value(&node_str);

    let mut out = String::with_capacity(4096);

    // 1. Operation & compilation metrics (aggregated from in-memory ring)
    format_operation_metrics(&mut out, metrics, &node);

    // 2. Host & gateway process resource metrics (from latest snapshot)
    if let Some(snapshot) = metrics.latest_snapshot() {
        format_snapshot_metrics(&mut out, &snapshot, &node);
    }

    // 3. Discovered toolchain inventory
    if let Some(inventory) = metrics.toolchain_inventory() {
        format_inventory_metrics(&mut out, &inventory, &node);
    }

    out
}
