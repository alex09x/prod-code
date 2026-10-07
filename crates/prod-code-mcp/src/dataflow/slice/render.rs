/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::line_analyzer::ParsedLine;
use super::super::types::{SliceCompleteness, SliceStatement};
use std::collections::{BTreeMap, BTreeSet};
use std::format;

pub(super) struct SliceRenderInput<'a> {
    pub(super) function_name: &'a str,
    pub(super) file_rel: &'a str,
    pub(super) start_line: u32,
    pub(super) end_line: u32,
    pub(super) target_line: u32,
    pub(super) parsed_lines: Vec<ParsedLine>,
    pub(super) retained_indices: BTreeSet<usize>,
    pub(super) statement_reasons: BTreeMap<usize, String>,
    pub(super) completeness: &'a SliceCompleteness,
}

pub(super) fn format_slice_statements(
    input: SliceRenderInput<'_>,
) -> (Vec<SliceStatement>, String, usize, usize, f64) {
    let SliceRenderInput {
        function_name,
        file_rel,
        start_line,
        end_line,
        target_line,
        parsed_lines,
        retained_indices,
        statement_reasons,
        completeness,
    } = input;
    let mut slice_statements: Vec<SliceStatement> = Vec::new();
    let mut formatted = String::new();

    let total_lines = parsed_lines.len();
    let retained_count = retained_indices.len();
    let reduction_percent = if total_lines > 0 {
        100.0 - (retained_count as f64 * 100.0 / total_lines as f64)
    } else {
        0.0
    };

    formatted.push_str(&format!(
        "// === INTRA-FUNCTION DATA-FLOW SLICE: `{}` ({}:{}-{}) ===\n",
        function_name, file_rel, start_line, end_line
    ));
    formatted.push_str(&format!(
        "// Completeness: {} | Target: line {} | Retained: {}/{} lines ({:.0}% reduction)\n",
        completeness.label(),
        target_line,
        retained_count,
        total_lines,
        reduction_percent
    ));
    if let SliceCompleteness::Incomplete { ref gap } = *completeness {
        formatted.push_str(&format!("// Missing evidence gap: {gap}\n"));
    }
    formatted.push_str(
        "// ============================================================================\n",
    );

    let mut prev_idx: Option<usize> = None;

    for &idx in &retained_indices {
        let pl = &parsed_lines[idx];

        if let Some(prev) = prev_idx {
            let omitted = idx.saturating_sub(prev + 1);
            if omitted > 0 {
                formatted.push_str(&format!(
                    "{}    // ... [sliced away {} statement(s) not affecting target] ...\n",
                    pl.indent, omitted
                ));
            }
        }
        prev_idx = Some(idx);

        let reason = statement_reasons
            .get(&idx)
            .cloned()
            .unwrap_or_else(|| "Retained statement".to_string());

        slice_statements.push(SliceStatement {
            line: pl.line,
            text: pl.trimmed.clone(),
            indent: pl.indent.clone(),
            reason: reason.clone(),
            is_control: pl.is_control,
            defined_vars: pl.defined_vars.clone(),
            used_vars: pl.used_vars.clone(),
        });

        formatted.push_str(&format!(
            "{:<4} | {}    // [{}]\n",
            pl.line, pl.raw_text, reason
        ));
    }
    (
        slice_statements,
        formatted,
        total_lines,
        retained_count,
        reduction_percent,
    )
}
