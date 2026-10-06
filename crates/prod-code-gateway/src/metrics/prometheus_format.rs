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
//!
//! Transforms gateway in-memory operations, periodic resource snapshots, and
//! toolchain inventory into standard Prometheus metric families with typed
//! HELP and TYPE metadata and low-cardinality labels.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::Metrics;

/// Escapes a label value according to the Prometheus exposition format specification:
/// backslashes, double quotes, and line feeds are escaped.
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

    // 1. Operation metrics (aggregated from in-memory ring)
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

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct OpKey {
    category: String,
    method: String,
    engine: String,
    status: &'static str,
    error_class: String,
}

#[derive(Default)]
struct OpAgg {
    count: u64,
    duration_ms: u64,
    items: u64,
    bytes: u64,
}

fn format_operation_metrics(out: &mut String, metrics: &Metrics, node: &str) {
    let mut agg_map: BTreeMap<OpKey, OpAgg> = BTreeMap::new();

    metrics.for_each_ring_event(|ev| {
        let status = if ev.ok { "ok" } else { "error" };
        let category = if !ev.kind.is_empty() {
            ev.kind.to_string()
        } else {
            "lsp".to_string()
        };
        let method = if !ev.method.is_empty() {
            ev.method.clone()
        } else if !ev.command.is_empty() {
            super::command_method(&ev.command)
        } else {
            "unknown".to_string()
        };
        let engine = if !ev.engine.is_empty() {
            ev.engine.clone()
        } else {
            "none".to_string()
        };
        let error_class = if !ev.ok {
            ev.error_class
                .clone()
                .unwrap_or_else(|| super::classify_error(&ev.command, ev.exit_code).to_string())
        } else {
            String::new()
        };

        let key = OpKey {
            category,
            method,
            engine,
            status,
            error_class,
        };

        let entry = agg_map.entry(key).or_default();
        entry.count += 1;
        entry.duration_ms += ev.duration_ms;
        entry.items += ev.items;
        entry.bytes += ev.bytes;
    });

    if agg_map.is_empty() {
        return;
    }

    // prod_code_operations_total
    let _ = writeln!(
        out,
        "# HELP prod_code_operations_total Total number of operations processed by the gateway.\n# TYPE prod_code_operations_total counter"
    );
    for (k, agg) in &agg_map {
        let cat = escape_label_value(&k.category);
        let meth = escape_label_value(&k.method);
        let eng = escape_label_value(&k.engine);
        if k.error_class.is_empty() {
            let _ = writeln!(
                out,
                "prod_code_operations_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\"}} {}",
                k.status, agg.count
            );
        } else {
            let err = escape_label_value(&k.error_class);
            let _ = writeln!(
                out,
                "prod_code_operations_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\",error_class=\"{err}\"}} {}",
                k.status, agg.count
            );
        }
    }

    // prod_code_operation_duration_seconds_total
    let _ = writeln!(
        out,
        "# HELP prod_code_operation_duration_seconds_total Total duration of completed operations in seconds.\n# TYPE prod_code_operation_duration_seconds_total counter"
    );
    for (k, agg) in &agg_map {
        let cat = escape_label_value(&k.category);
        let meth = escape_label_value(&k.method);
        let eng = escape_label_value(&k.engine);
        let secs = agg.duration_ms as f64 / 1000.0;
        let _ = writeln!(
            out,
            "prod_code_operation_duration_seconds_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\"}} {:.6}",
            k.status, secs
        );
    }

    // prod_code_operation_items_total
    let _ = writeln!(
        out,
        "# HELP prod_code_operation_items_total Total items processed or returned by operations.\n# TYPE prod_code_operation_items_total counter"
    );
    for (k, agg) in &agg_map {
        if agg.items > 0 {
            let cat = escape_label_value(&k.category);
            let meth = escape_label_value(&k.method);
            let eng = escape_label_value(&k.engine);
            let _ = writeln!(
                out,
                "prod_code_operation_items_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\"}} {}",
                agg.items
            );
        }
    }

    // prod_code_operation_bytes_total
    let _ = writeln!(
        out,
        "# HELP prod_code_operation_bytes_total Total bytes transferred or produced by operations.\n# TYPE prod_code_operation_bytes_total counter"
    );
    for (k, agg) in &agg_map {
        if agg.bytes > 0 {
            let cat = escape_label_value(&k.category);
            let meth = escape_label_value(&k.method);
            let eng = escape_label_value(&k.engine);
            let _ = writeln!(
                out,
                "prod_code_operation_bytes_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\"}} {}",
                agg.bytes
            );
        }
    }
}

fn format_snapshot_metrics(out: &mut String, s: &prod_code_protocol::HostSnapshot, node: &str) {
    if let Some(cpu_pct) = s.cpu_usage_pct() {
        let ratio = cpu_pct / 100.0;
        let _ = writeln!(
            out,
            "# HELP prod_code_gateway_cpu_usage_ratio Current CPU utilization ratio (0.0 - 1.0).\n# TYPE prod_code_gateway_cpu_usage_ratio gauge\nprod_code_gateway_cpu_usage_ratio{{node=\"{node}\"}} {:.4}",
            ratio
        );
    }

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_cpu_count Logical CPU core count on the host.\n# TYPE prod_code_gateway_cpu_count gauge\nprod_code_gateway_cpu_count{{node=\"{node}\"}} {}",
        s.cpu_count
    );

    if let Some(load_avg) = s.load_average_1m() {
        let _ = writeln!(
            out,
            "# HELP prod_code_gateway_load_average_1m 1-minute system load average.\n# TYPE prod_code_gateway_load_average_1m gauge\nprod_code_gateway_load_average_1m{{node=\"{node}\"}} {:.2}",
            load_avg
        );
    }

    if let Some(rss) = s.process_rss_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_gateway_process_rss_bytes Resident Set Size (RSS) of the gateway process in bytes.\n# TYPE prod_code_gateway_process_rss_bytes gauge\nprod_code_gateway_process_rss_bytes{{node=\"{node}\"}} {}",
            rss
        );
    }

    if let Some(avail) = s.host_memory_available_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_host_memory_available_bytes Available host memory without swapping in bytes.\n# TYPE prod_code_host_memory_available_bytes gauge\nprod_code_host_memory_available_bytes{{node=\"{node}\"}} {}",
            avail
        );
    }

    if let Some(total) = s.host_memory_total_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_host_memory_total_bytes Total physical host memory in bytes.\n# TYPE prod_code_host_memory_total_bytes gauge\nprod_code_host_memory_total_bytes{{node=\"{node}\"}} {}",
            total
        );
    }

    if let Some(storage_free) = s.storage_free_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_host_storage_free_bytes Free storage on the workspaces filesystem in bytes.\n# TYPE prod_code_host_storage_free_bytes gauge\nprod_code_host_storage_free_bytes{{node=\"{node}\"}} {}",
            storage_free
        );
    }

    if let Some(free_millis) = s.storage_free_millis {
        let free_ratio = free_millis as f64 / 1000.0;
        let _ = writeln!(
            out,
            "# HELP prod_code_host_storage_free_ratio Free storage ratio on the workspaces filesystem (0.0 - 1.0).\n# TYPE prod_code_host_storage_free_ratio gauge\nprod_code_host_storage_free_ratio{{node=\"{node}\"}} {:.4}",
            free_ratio
        );
    }

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_active_sessions Number of active client connections or sessions.\n# TYPE prod_code_gateway_active_sessions gauge\nprod_code_gateway_active_sessions{{node=\"{node}\"}} {}",
        s.active_sessions
    );

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_active_queries Number of queries currently being executed.\n# TYPE prod_code_gateway_active_queries gauge\nprod_code_gateway_active_queries{{node=\"{node}\"}} {}",
        s.active_queries
    );

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_running_commands Number of remote exec commands currently running.\n# TYPE prod_code_gateway_running_commands gauge\nprod_code_gateway_running_commands{{node=\"{node}\"}} {}",
        s.running_commands
    );

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_workspace_count Number of currently loaded workspaces.\n# TYPE prod_code_gateway_workspace_count gauge\nprod_code_gateway_workspace_count{{node=\"{node}\"}} {}",
        s.workspace_count
    );

    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_engine_count Number of advertised language engines.\n# TYPE prod_code_gateway_engine_count gauge\nprod_code_gateway_engine_count{{node=\"{node}\"}} {}",
        s.engine_count
    );

    let ver = escape_label_value(&s.version);
    let commit = escape_label_value(&s.git_commit);
    let plat = escape_label_value(&s.platform);
    let _ = writeln!(
        out,
        "# HELP prod_code_gateway_info Gateway version, commit, and platform information.\n# TYPE prod_code_gateway_info gauge\nprod_code_gateway_info{{node=\"{node}\",version=\"{ver}\",git_commit=\"{commit}\",platform=\"{plat}\"}} 1"
    );
}

fn format_inventory_metrics(
    out: &mut String,
    inv: &prod_code_protocol::ToolchainInventory,
    node: &str,
) {
    if inv.engines.is_empty() {
        return;
    }

    let _ = writeln!(
        out,
        "# HELP prod_code_toolchain_info Discovered compiler and toolchain versions on the host.\n# TYPE prod_code_toolchain_info gauge"
    );

    for eng in &inv.engines {
        let eng_name = escape_label_value(&eng.engine);
        for tc in &eng.toolchains {
            let tool = escape_label_value(&tc.tool);
            let ver = escape_label_value(&tc.version);
            let _ = writeln!(
                out,
                "prod_code_toolchain_info{{node=\"{node}\",engine=\"{eng_name}\",tool=\"{tool}\",version=\"{ver}\"}} 1"
            );
        }
    }
}
