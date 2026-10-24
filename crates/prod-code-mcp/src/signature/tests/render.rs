/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use crate::signature::types::SignatureChange;

fn change() -> SignatureChange {
    SignatureChange {
        symbol: "f".into(),
        root: "/nonexistent".into(),
        file: "src/lib.rs".into(),
        old_signature: "a: u8".into(),
        new_signature: "b: u8".into(),
        rule: String::new(),
        rewritten: vec![],
        unmatched: vec![],
        unexpected: vec![],
        diagnostics: vec![],
        applied: false,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: vec![],
    }
}

#[test]
fn a_plain_change_reports_before_after_and_that_nothing_was_written() {
    assert_eq!(
        change().render(100),
        "`f` (src/lib.rs)\n\n- was: (a: u8)\n- now: (b: u8)\n\n0 changed line(s) in 0 file(s)\n\n\nthe proposal passes validation: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"
    );
}

#[test]
fn modifiers_awaits_leftovers_and_failures_are_listed() {
    let dir = tempfile::Builder::new().prefix("sr").tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "fn f(a: u8) {}\nfn g() {}\n").unwrap();
    let mut c = change();
    c.root = dir.path().to_path_buf();
    c.rewritten = vec![(
        path.to_string_lossy().into_owned(),
        "fn f(b: u8) {}\nfn g() {}\n".into(),
    )];
    c.rule = "f($A) -> f($A)".into();
    c.returns = Some(("()".into(), "u8".into()));
    c.visibility = Some(("private".into(), "pub".into()));
    c.asyncness = Some((false, true));
    c.not_async = vec!["lib.rs:2:1".into()];
    c.unmatched = vec!["lib.rs:9:9".into()];
    c.unexpected = vec!["lib.rs:7:7".into()];
    c.diagnostics = vec!["mismatched types (lib.rs:1:1)".into()];
    let out = c.render(10_000);
    assert!(out.starts_with("`f` (src/lib.rs)\n\n- was: (a: u8)\n- now: (b: u8)\n- returns: `()` → `u8`\n- visibility: `private` → `pub`\n- not async → async: every call gains `.await`\n- lib.rs:2:1: awaits from a function that is not `async`\n- call sites: `f($A) -> f($A)`\n\n2 changed line(s) in 1 file(s)\n\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1,2 +1,2 @@\n-fn f(a: u8) {}\n+fn f(b: u8) {}\n fn g() {}\n"));
    assert!(out.contains("\nnot rewritten (1 reference(s) the rule did not match — a call through a function pointer, a macro, or a spelling structural search cannot see):\n  lib.rs:9:9\n"));
    assert!(out.contains(
        "\nchanged without being a known reference (1), check these by hand:\n  lib.rs:7:7\n"
    ));
    assert!(out.contains("\nthe proposal fails validation:\n  mismatched types (lib.rs:1:1)\n"));
    assert!(out.ends_with("\nnothing was written, and `apply` writes nothing while a reference is not rewritten or a change is not a reference, `force` or not\n"));

    c.asyncness = Some((true, false));
    c.applied = true;
    let out = c.render(10);
    assert!(out.contains("- async → not async: every call loses `.await`\n"));
    assert!(out.contains("file(s)\n\n--- a/lib.\n… diff truncated\n"));
    assert!(out.ends_with("\n[applied to 1 file(s)]\n"));
}

#[test]
fn an_incomplete_or_awaiting_change_is_not_writable_even_when_forced() {
    let mut c = change();
    c.ensure_writable(false).unwrap();
    c.unmatched = vec!["u1".into()];
    c.unexpected = vec!["x1".into(), "x2".into()];
    let want = "the change to `f` is not complete: 1 reference(s) were not rewritten and 2 change(s) are not a reference; nothing was written, and `force` does not override this:\n  not rewritten: u1\n  changed without being a reference: x1\n  changed without being a reference: x2";
    for force in [false, true] {
        assert_eq!(c.ensure_writable(force).unwrap_err().to_string(), want);
    }
    c.unmatched.clear();
    c.unexpected = vec!["x1".into()];
    assert!(
        c.ensure_writable(true)
            .unwrap_err()
            .to_string()
            .starts_with("the change to `f` is not complete: 0 reference(s)")
    );

    let mut c = change();
    c.not_async = vec!["a.rs:1:1".into(), "b.rs:2:2".into()];
    assert_eq!(
        c.ensure_writable(false).unwrap_err().to_string(),
        "2 call(s) would `.await` from a function that is not `async`; nothing was written. Make those callers `async` first, or pass `force: true`:\n  a.rs:1:1\n  b.rs:2:2"
    );
    c.ensure_writable(true).unwrap();
}
