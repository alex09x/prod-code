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
fn coverage_constructor_report_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.rs");
    std::fs::write(&path, "old\n").unwrap();
    let mut result = ReplaceConstructorResult {
        type_name: "Item".into(),
        root: dir.path().into(),
        file: "input.rs".into(),
        mode: ReplaceMode::Factory,
        target_name: "create".into(),
        declared_fields: vec![],
        instantiations_rewritten: 1,
        blocked: vec![],
        unmatched: vec![],
        rewritten: vec![(path.to_str().unwrap().into(), "new\n".into())],
        diagnostics: vec![],
        applied: false,
        language: "rust".into(),
    };
    let head = "`Item` (input.rs) — replaced constructor with factory\n\n- target: `create` (factory)\n- declared fields (0): none\n- 1 instantiation(s) rewritten across 1 file(s)\n\n2 changed line(s) in 1 file(s)\n\n";
    assert_eq!(
        result.render(1000),
        format!(
            "{head}--- a/input.rs\n+++ b/input.rs\n@@ -1 +1 @@\n-old\n+new\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"
        )
    );
    result.mode = ReplaceMode::Builder;
    result.declared_fields = vec!["x".into(), "y".into()];
    result.blocked = vec!["rest pattern".into()];
    result.unmatched = vec!["used as value".into()];
    result.diagnostics = vec!["type mismatch".into()];
    result.applied = true;
    assert_eq!(
        result.render(0),
        "`Item` (input.rs) — replaced constructor with builder\n\n- target: `create` (builder)\n- declared fields (2): x, y\n- 1 instantiation(s) rewritten across 1 file(s)\n\n2 changed line(s) in 1 file(s)\n\n\n… diff truncated\n\n1 instantiation(s) could not be safely rewritten:\n  rest pattern\n\nnot rewritten (1 reference(s)):\n  used as value\n\nthe analyzer rejects the result:\n  type mismatch\n\n[applied to 1 file(s)]\n"
    );
    assert_eq!(display(dir.path(), &path), "input.rs");
    assert_eq!(
        display(Path::new("root"), Path::new("outside.rs")),
        "outside.rs"
    );
}
