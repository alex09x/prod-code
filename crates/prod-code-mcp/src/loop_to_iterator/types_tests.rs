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
fn coverage_b2_loop_report_before_and_after_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loop.rs");
    std::fs::write(&path, "old\n").unwrap();
    let mut result = Rewritten {
        statement: "  use an iterator  ".into(),
        root: dir.path().into(),
        file: "loop.rs".into(),
        new_text: "new\n".into(),
        diagnostics: vec![],
        applied: false,
    };
    let diff = "use an iterator\n\n--- a/loop.rs\n+++ b/loop.rs\n@@ -1 +1 @@\n-old\n+new\n";
    assert_eq!(
        result.render(),
        format!(
            "{diff}\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make this edit\n"
        )
    );
    std::fs::write(&path, "new\n").unwrap();
    crate::refactor::remember_applied_multi(&[(path, Some(b"old\n".to_vec()))]);
    result.applied = true;
    result.diagnostics = vec!["bad type".into(), "missing name".into()];
    assert_eq!(
        result.render(),
        format!(
            "{diff}\nthe analyzer rejects the result:\n  bad type\n  missing name\n\n[applied]\n"
        )
    );
}
