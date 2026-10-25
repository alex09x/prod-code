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

#[test]
fn coverage_b2_delegation_report() {
    let mut result = ReplaceInheritanceResult {
        sub_type: "Queue".into(),
        base_type: "Store".into(),
        field_name: "store".into(),
        forwarded_methods: vec![],
        files_modified: vec!["queue.py".into()],
        overlays: vec![],
        diff: String::new(),
        applied: false,
        verified: false,
        diagnostics: vec![],
    };
    let prefix = "`Queue` — replaced inheritance from `Store` with delegation via `store`\n";
    assert_eq!(
        result.render(0),
        format!(
            "{prefix}- forwarded methods (0): none\n- files modified (1): queue.py\n- applied: false\n- verified: false\n"
        )
    );
    result.forwarded_methods = vec!["push".into(), "pop".into()];
    result.diagnostics = vec!["unknown Store".into(), "missing push".into()];
    result.applied = true;
    result.verified = true;
    result.diff = "-old\n+new\n".into();
    let summary = format!(
        "{prefix}- forwarded methods (2): push, pop\n- files modified (1): queue.py\n- applied: true\n- verified: true\n- diagnostics (2):\n  • unknown Store\n  • missing push\n\nDiff:\n```diff\n"
    );
    assert_eq!(result.render(10), format!("{summary}-old\n+new\n```\n"));
    assert_eq!(
        result.render(4),
        format!("{summary}-old\n... [truncated]\n```\n")
    );
    result.diff = "éclair".into();
    assert_eq!(
        result.render(1),
        format!("{summary}\n... [truncated]\n```\n")
    );
}
