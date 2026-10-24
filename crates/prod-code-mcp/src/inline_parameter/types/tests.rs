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
fn coverage_inline_report_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("input.py");
    std::fs::write(&file, "old\n").unwrap();
    let mut result = InlinedParameter {
        function: "work".into(), parameter: "x".into(), value: "7".into(),
        root: dir.path().into(), file: "input.py".into(), rewritten_calls: 2,
        unmatched: vec![], rewritten: vec![(file.to_str().unwrap().into(), "new\n".into())],
        diagnostics: vec![], applied: false,
    };
    let head = "`x` of `work` (input.py)\n\n- every call passes `7`: it is bound at the top of the body\n- 2 call(s) lose the argument\n\n2 changed line(s) in 1 file(s)\n\n";
    assert_eq!(result.render(1000), format!("{head}--- a/input.py\n+++ b/input.py\n@@ -1 +1 @@\n-old\n+new\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"));
    result.unmatched = vec!["input.py:3: used as value".into()];
    result.diagnostics = vec!["unknown name".into()];
    result.applied = true;
    assert_eq!(result.render(0), format!("{head}\n… diff truncated\n\nnot a call with this argument (1); nothing is written while any remains:\n  input.py:3: used as value\n\nthe analyzer rejects the result:\n  unknown name\n\n[applied to 1 file(s)]\n"));
    assert_eq!(display(dir.path(), &file), "input.py");
    assert_eq!(display(Path::new("root"), Path::new("outside.py")), "outside.py");
}
