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
fn coverage_wrapper_names_and_serialization() {
    for (text, wrapper, name, json, assist) in [
        (" OPTIONAL ", Wrapper::Option, "Option", "\"option\"", "wrap_return_type_in_option"),
        ("Expected", Wrapper::Result, "Result", "\"result\"", "wrap_return_type_in_result"),
        ("future", Wrapper::Promise, "Promise", "\"promise\"", "wrap_return_type_in_option"),
        ("PTR", Wrapper::Pointer, "Pointer", "\"pointer\"", "wrap_return_type_in_option"),
        (" Box<T> ", Wrapper::Custom("Box<T>".into()), "Box<T>", "\"Box<T>\"", "wrap_return_type_in_option"),
    ] {
        assert_eq!(Wrapper::parse(text).unwrap(), wrapper);
        assert_eq!(wrapper.name(), name);
        assert_eq!(serde_json::to_string(&wrapper).unwrap(), json);
        assert_eq!(wrapper.assist_id(), assist);
    }
    assert_eq!(Wrapper::parse(" ").unwrap_err().to_string(), "wrapper cannot be empty: use `option`, `result`, `promise`, `pointer`, or a custom envelope type name");
}

#[test]
fn coverage_wrap_report_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.ts");
    std::fs::write(&path, "old\n").unwrap();
    let mut result = WrappedReturn {
        function: "work".into(), root: dir.path().into(), file: "input.ts".into(),
        was: "number".into(), now: "Promise<number>".into(), propagated: 1,
        blocked: vec!["caller".into()], unmatched: vec!["value".into()],
        rewritten: vec![(path.to_str().unwrap().into(), "new\n".into())],
        diagnostics: vec!["error".into()], applied: true,
    };
    for (now, note, target) in [
        ("Promise<number>", " with `await`", "a `Promise`"),
        ("Option<number>", " with `?`", "an `Option`"),
        ("Result<number>", " with `?`", "a `Result`"),
        ("Box<number>", "", "Box<number>"),
    ] {
        result.now = now.into();
        assert_eq!(result.render(0), format!("`work` (input.ts)\n\n- returned: `number`\n- now returns: `{now}`\n- 1 call site(s) propagate{note}\n\n2 changed line(s) in 1 file(s)\n\n\n… diff truncated\n\n1 call site(s) cannot propagate: the calling function does not return {target}. Each needs a decision — unwrap, match, or wrap that caller too:\n  caller\n\nnot rewritten (1 reference(s) this could not read as a call):\n  value\n\nthe analyzer rejects the result:\n  error\n\n[applied to 1 file(s)]\n"));
    }
    result.blocked.clear(); result.unmatched.clear(); result.diagnostics.clear(); result.applied = false;
    assert_eq!(result.render(1000), "`work` (input.ts)\n\n- returned: `number`\n- now returns: `Box<number>`\n- 1 call site(s) propagate\n\n2 changed line(s) in 1 file(s)\n\n--- a/input.ts\n+++ b/input.ts\n@@ -1 +1 @@\n-old\n+new\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n");
}
