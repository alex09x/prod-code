/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Host snapshot and toolchain inventory Prometheus exposition formatting.

use super::escape_label_value;
use prod_code_protocol::{HostSnapshot, ToolchainInventory};
use std::fmt::Write;

pub fn format_snapshot_metrics(out: &mut String, s: &HostSnapshot, node: &str) {
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

    if let (Some(total), Some(avail)) = (s.host_memory_total_bytes, s.host_memory_available_bytes) {
        let used = total.saturating_sub(avail);
        let _ = writeln!(
            out,
            "# HELP prod_code_host_memory_used_bytes Estimated used host memory in bytes.\n# TYPE prod_code_host_memory_used_bytes gauge\nprod_code_host_memory_used_bytes{{node=\"{node}\"}} {}",
            used
        );
    }

    if let Some(storage_total) = s.storage_total_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_host_storage_total_bytes Total filesystem storage on the workspaces volume in bytes.\n# TYPE prod_code_host_storage_total_bytes gauge\nprod_code_host_storage_total_bytes{{node=\"{node}\"}} {}",
            storage_total
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

    if let Some(ws_storage) = s.workspace_storage_bytes {
        let _ = writeln!(
            out,
            "# HELP prod_code_workspaces_storage_bytes Total disk space used by workspace directories in bytes.\n# TYPE prod_code_workspaces_storage_bytes gauge\nprod_code_workspaces_storage_bytes{{node=\"{node}\"}} {}",
            ws_storage
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

pub fn format_inventory_metrics(out: &mut String, inv: &ToolchainInventory, node: &str) {
    if inv.engines.is_empty() {
        return;
    }

    let _ = writeln!(
        out,
        "# HELP prod_code_toolchain_info Installed host compiler and toolchain versions.\n# TYPE prod_code_toolchain_info gauge"
    );

    for eng in &inv.engines {
        let engine_name = escape_label_value(&eng.engine);
        for tv in &eng.toolchains {
            let tool_name = escape_label_value(&tv.tool);
            let version = escape_label_value(&tv.version);
            let _ = writeln!(
                out,
                "prod_code_toolchain_info{{node=\"{node}\",engine=\"{engine_name}\",tool=\"{tool_name}\",version=\"{version}\"}} 1"
            );
        }
    }
}
