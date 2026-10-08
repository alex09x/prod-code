/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{Diagnostic, truncate_to_boundary};

const DIAGNOSTIC_TRUNCATED_MARKER: &str = " [... diagnostic truncated]";
const OMITTED_DIAGNOSTIC_MARKER_RESERVE: usize = 80;

pub(crate) fn append_diagnostics(
    out: &mut String,
    diagnostics: &[Diagnostic],
    max_items: usize,
    budget: usize,
) -> usize {
    let mut rendered = 0;
    let diagnostic_budget = budget.saturating_sub(OMITTED_DIAGNOSTIC_MARKER_RESERVE);

    for diagnostic in diagnostics.iter().take(max_items) {
        let remaining = diagnostic_budget.saturating_sub(out.len());
        let line_overhead = "  ".len() + "\n".len();
        if remaining <= line_overhead {
            break;
        }

        let body_budget = remaining - line_overhead;
        let text = diagnostic.render();
        if text.len() > body_budget {
            if body_budget <= DIAGNOSTIC_TRUNCATED_MARKER.len() {
                break;
            }
            let content_budget = body_budget - DIAGNOSTIC_TRUNCATED_MARKER.len();
            out.push_str("  ");
            out.push_str(truncate_to_boundary(&text, content_budget));
            out.push_str(DIAGNOSTIC_TRUNCATED_MARKER);
            out.push('\n');
            rendered += 1;
            break;
        }

        out.push_str("  ");
        out.push_str(&text);
        out.push('\n');
        rendered += 1;
    }

    if diagnostics.len() > rendered {
        out.push_str(&format!(
            "  ... {} more diagnostic(s)\n",
            diagnostics.len() - rendered
        ));
    }
    rendered
}
