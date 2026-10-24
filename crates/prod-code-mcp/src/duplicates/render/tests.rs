/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;
use crate::duplicates::types::{CloneGroup, CodeCloneOccurrence, DuplicateOptions};

fn report() -> DuplicationReport {
    DuplicationReport {
        total_files_scanned: 2,
        total_lines_scanned: 100,
        total_clone_groups: 0,
        duplicated_lines: 0,
        duplication_percentage: 0.0,
        approximate: false,
        groups: vec![],
    }
}

#[test]
fn empty_report_and_options() {
    assert_eq!(
        format_duplication_report(&report()),
        "⚡ prod-code Clone & Duplication Harvester Report\n────────────────────────────────────────────────────\nFiles Scanned: 2 | Lines: 100 | Clone Groups: 0 | Duplication: 0.0%\n\n✓ No duplicate code blocks detected exceeding threshold.\n"
    );
    let options = DuplicateOptions::default();
    assert_eq!(
        (
            options.min_lines,
            options.parameterized,
            options.type3,
            options.max_groups
        ),
        (6, true, false, 20)
    );
}

#[test]
fn clone_groups_preview_and_approximation() {
    let mut r = report();
    r.approximate = true;
    r.total_clone_groups = 2;
    r.duplication_percentage = 12.34;
    r.groups = vec![
        CloneGroup {
            id: 7,
            clone_type: "Type-1".into(),
            line_count: 5,
            occurrences: vec![
                CodeCloneOccurrence {
                    file: "a.rs".into(),
                    start_line: 1,
                    end_line: 5,
                    snippet: "α\r\nb\nc\nd\ne".into(),
                },
                CodeCloneOccurrence {
                    file: "b.rs".into(),
                    start_line: 9,
                    end_line: 13,
                    snippet: "ignored".into(),
                },
            ],
        },
        CloneGroup {
            id: 8,
            clone_type: "Type-2".into(),
            line_count: 0,
            occurrences: vec![],
        },
    ];
    assert_eq!(
        format_duplication_report(&r),
        "⚡ prod-code Clone & Duplication Harvester Report\n────────────────────────────────────────────────────\nFiles Scanned: 2 | Lines: 100 | Clone Groups: 2 | Duplication: 12.3%\n⚠️ High repetition: candidate comparisons were capped; results are approximate.\n\nDiscovered Clone Groups:\n\n[Clone Group #7] 5 lines | 2 occurrences (Type-1)\n  • Occurrence 1: a.rs:1-5\n  • Occurrence 2: b.rs:9-13\n  Preview:\n    │ α\n    │ b\n    │ c\n    │ d\n    │ …\n  💡 Recommendation: Fold into a shared function using `code_extract_function`.\n\n[Clone Group #8] 0 lines | 0 occurrences (Type-2)\n  💡 Recommendation: Fold into a shared function using `code_extract_function`.\n"
    );
    r.groups.truncate(1);
    r.groups[0].occurrences.truncate(1);
    r.groups[0].occurrences[0].snippet = "one".into();
    assert_eq!(
        format_duplication_report(&r)
            .split("  Preview:\n")
            .nth(1)
            .unwrap(),
        "    │ one\n  💡 Recommendation: Fold into a shared function using `code_extract_function`.\n"
    );
}
