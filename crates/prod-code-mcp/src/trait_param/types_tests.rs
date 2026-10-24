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
fn coverage_trait_report_refusals_and_diff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    std::fs::write(&path, "old\n").unwrap();
    let mut report = TraitParameter {
        method: "run".into(), parameter: "extra".into(), index: 1, root: dir.path().into(),
        declarations: vec!["sample.rs:1".into()], calls: 2,
        blocked: vec!["argument has effects".into()], unmatched: vec!["sample.rs:4 value use".into()],
        rewritten: vec![(path.to_string_lossy().into_owned(), "new\n".into())],
        diagnostics: vec!["type mismatch".into()], applied: false,
    };
    let prefix = "the parameter `extra` (#2 after the receiver) leaves `run`: 1 declaration(s), 2 call(s)\n- sample.rs:1\n\nnothing may be written while:\n  argument has effects\n\nnot rewritten; nothing is written while any remains, forced or not:\n  sample.rs:4 value use\n\n";
    let suffix = "\nthe analyzer rejects the result:\n  type mismatch\n\nnothing was written\n";
    assert_eq!(report.render(0), format!("{prefix}\n… diff truncated\n{suffix}"));
    assert_eq!(report.render(usize::MAX), format!("{prefix}--- a/sample.rs\n+++ b/sample.rs\n@@ -1 +1 @@\n-old\n+new\n{suffix}"));
    report.rewritten.clear(); report.blocked.clear(); report.unmatched.clear(); report.diagnostics.clear(); report.applied = true;
    assert_eq!(report.render(0), "the parameter `extra` (#2 after the receiver) leaves `run`: 1 declaration(s), 2 call(s)\n- sample.rs:1\n\n\nthe analyzer accepts the result: 0 errors\n\n[applied]\n");
    assert_eq!(display(dir.path(), &path), "sample.rs");
    assert_eq!(display(Path::new("different"), Path::new("sample.rs")), "sample.rs");
}
