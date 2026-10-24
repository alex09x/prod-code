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
fn coverage_move_report_refusals_diagnostics_and_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    std::fs::write(&path, "old\n").unwrap();
    let mut report = MovedMethod {
        method: "run".into(),
        from_type: "Source".into(),
        to_type: "Target".into(),
        root: dir.path().into(),
        signature: "fn run(&self)".into(),
        calls: 2,
        blocked: vec!["receiver has effects".into()],
        unmatched: vec!["sample.rs:3 method value".into()],
        rewritten: vec![(path.to_string_lossy().into_owned(), "new\n".into())],
        diagnostics: vec!["cannot find helper".into()],
        applied: false,
    };
    let prefix = "`Source::run` is now `Target::run`: `fn run(&self)`; 2 call(s) rewritten\n\nnothing may be written while:\n  receiver has effects\n\nnot rewritten; nothing is written while any remains, forced or not:\n  sample.rs:3 method value\n\n";
    let suffix = "\nthe analyzer rejects the result:\n  cannot find helper\n\na name the body uses resolves where `run` was and not where it goes: import it in the new file, or spell its path, and run this again\n\nnothing was written; pass `apply: true` to make these edits\n";
    assert_eq!(
        report.render(0),
        format!("{prefix}\n… diff truncated\n{suffix}")
    );
    assert_eq!(
        report.render(usize::MAX),
        format!("{prefix}--- a/sample.rs\n+++ b/sample.rs\n@@ -1 +1 @@\n-old\n+new\n{suffix}")
    );
    report.rewritten.clear();
    report.blocked.clear();
    report.unmatched.clear();
    report.diagnostics.clear();
    report.applied = true;
    assert_eq!(
        report.render(0),
        "`Source::run` is now `Target::run`: `fn run(&self)`; 2 call(s) rewritten\n\n\nthe analyzer accepts the result: 0 errors\n\n[applied]\n"
    );
}
