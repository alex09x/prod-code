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
use prod_code_testkit::Workspace;

#[test]
fn preview_diff_reads_the_original_file_without_writing() {
    let ws = Workspace::new(&[("model.rs", "struct Before;\n")]);
    let file = ws.path("model.rs");
    let extracted = Extracted {
        helper: "Details".into(),
        field: "details".into(),
        fields: vec!["name".into()],
        methods: vec![],
        root: ws.root(),
        rewritten: vec![(
            file.to_string_lossy().into_owned(),
            "struct After;\n".into(),
        )],
        accesses: 0,
        unmatched: vec![],
        diagnostics: vec![],
        applied: false,
    };
    assert_eq!(
        extracted.render(),
        "`details: Details` holds name; 0 method(s) moved, 0 access(es) rerouted\n\n--- a/model.rs\n+++ b/model.rs\n@@ -1 +1 @@\n-struct Before;\n+struct After;\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make this edit\n"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "struct Before;\n");
}

#[test]
fn render_reports_unmatched_references_and_all_diagnostics() {
    let mut extracted = Extracted {
        helper: "Details".into(),
        field: "details".into(),
        fields: vec!["name".into(), "size".into()],
        methods: vec!["size".into()],
        root: PathBuf::new(),
        rewritten: vec![],
        accesses: 2,
        unmatched: vec!["unresolved access".into()],
        diagnostics: vec!["unknown type".into(), "missing name".into()],
        applied: true,
    };
    assert_eq!(
        extracted.render(),
        "`details: Details` holds name, size; 1 method(s) moved, 2 access(es) rerouted\n\nunresolved references (the edit is incomplete):\n  • unresolved access\n\nthe analyzer rejects the result:\n  unknown type\n  missing name\n\n[applied]\n"
    );
    extracted.applied = false;
    extracted.unmatched.clear();
    extracted.diagnostics.clear();
    assert_eq!(
        extracted.render(),
        "`details: Details` holds name, size; 1 method(s) moved, 2 access(es) rerouted\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make this edit\n"
    );
}
