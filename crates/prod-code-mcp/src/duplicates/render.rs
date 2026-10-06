/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::DuplicationReport;

/// Formats the duplication report into a clean readable summary.
pub fn format_duplication_report(report: &DuplicationReport) -> String {
    let mut out = String::new();
    out.push_str("⚡ prod-code Clone & Duplication Harvester Report\n");
    out.push_str("────────────────────────────────────────────────────\n");
    out.push_str(&format!(
        "Files Scanned: {} | Lines: {} | Clone Groups: {} | Duplication: {:.1}%\n",
        report.total_files_scanned,
        report.total_lines_scanned,
        report.total_clone_groups,
        report.duplication_percentage
    ));

    if report.approximate {
        out.push_str(
            "⚠️ High repetition: candidate comparisons were capped; results are approximate.\n",
        );
    }

    if report.groups.is_empty() {
        out.push_str("\n✓ No duplicate code blocks detected exceeding threshold.\n");
        return out;
    }

    out.push_str("\nDiscovered Clone Groups:\n");
    for group in &report.groups {
        out.push_str(&format!(
            "\n[Clone Group #{}] {} lines | {} occurrences ({})\n",
            group.id,
            group.line_count,
            group.occurrences.len(),
            group.clone_type
        ));
        for (i, occ) in group.occurrences.iter().enumerate() {
            out.push_str(&format!(
                "  • Occurrence {}: {}:{}-{}\n",
                i + 1,
                occ.file,
                occ.start_line,
                occ.end_line
            ));
        }

        // Show preview of first occurrence snippet
        if let Some(first) = group.occurrences.first() {
            out.push_str("  Preview:\n");
            for line in first.snippet.lines().take(4) {
                out.push_str(&format!("    │ {}\n", line));
            }
            if first.snippet.lines().count() > 4 {
                out.push_str("    │ …\n");
            }
        }
        out.push_str(
            "  💡 Recommendation: Fold into a shared function using `code_extract_function`.\n",
        );
    }

    out
}
