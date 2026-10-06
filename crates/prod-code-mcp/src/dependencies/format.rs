/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::DependencyGraphReport;

/// Formats a dependency report as an ASCII summary table and cycle diagnosis.
pub fn format_dependency_report(report: &DependencyGraphReport) -> String {
    let mut out = String::new();
    out.push_str("⚡ prod-code Architecture & Dependency Graph Report\n");
    out.push_str("────────────────────────────────────────────────────\n");
    out.push_str(&format!(
        "Scope: {} | Nodes: {} | Dependencies: {}\n",
        report.scope, report.total_nodes, report.total_edges
    ));

    if report.cycles_detected > 0 {
        let shown = report.cycles.len().min(25);
        out.push_str(&format!(
            "\n🚨 CYCLES DETECTED: {} circular dependency path(s) found:\n",
            report.cycles_detected
        ));
        for (i, cycle) in report.cycles.iter().take(shown).enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, cycle.join(" -> ")));
        }
        if report.cycles_detected > shown {
            out.push_str(&format!(
                "  … and {} more circular dependency path(s) truncated\n",
                report.cycles_detected - shown
            ));
        }
    } else {
        out.push_str(
            "\n✓ Zero circular dependencies detected. Architecture graph is a clean DAG.\n",
        );
    }

    out.push_str("\nTop Coupled Modules / Crates (by Afferent Coupling Ca):\n");
    out.push_str(&format!(
        "  {:<32} {:>5} {:>5} {:>7}\n",
        "Name", "Ca", "Ce", "Instab"
    ));
    out.push_str("  ────────────────────────────────────────────────────\n");

    for node in report.nodes.iter().take(15) {
        out.push_str(&format!(
            "  {:<32} {:>5} {:>5} {:>7.2}\n",
            node.name, node.afferent_coupling, node.efferent_coupling, node.instability
        ));
    }

    if !report.isolated_nodes.is_empty() {
        out.push_str(&format!(
            "\nIsolated (Leaf/Orphan) Nodes ({}): {}\n",
            report.isolated_nodes.len(),
            report
                .isolated_nodes
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    out
}
