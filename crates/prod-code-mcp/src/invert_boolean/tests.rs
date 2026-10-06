/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{call_start, negated_body, own_returns};
use super::types::Inverted;

#[test]
fn a_one_expression_body_is_negated_in_place() {
    assert_eq!(negated_body("\n    self.n > 0\n"), "\n    !(self.n > 0)\n");
}

#[test]
fn a_body_with_statements_is_negated_as_a_block_and_its_returns_one_by_one() {
    let inner = "\n    if n == 0 {\n        return true;\n    }\n    let m = n % 2;\n    m == 0\n";
    let out = negated_body(inner);
    assert!(out.contains("return !(true);"), "{out}");
    assert!(out.trim_start().starts_with("!{"), "{out}");
    assert!(out.contains("m == 0"), "{out}");
}

#[test]
fn a_return_inside_a_closure_or_an_async_block_is_not_the_functions() {
    let inner =
        "\n    let f = |x: u32| { return x > 1; };\n    let g = async { return 3; };\n    f(2)\n";
    assert!(own_returns(inner).is_empty(), "{:?}", own_returns(inner));
    let inner = "\n    fn helper() -> bool {\n        return true;\n    }\n    helper()\n";
    assert!(
        own_returns(inner).is_empty(),
        "a nested fn's return is its own"
    );
    let inner = "\n    if x { return false; }\n    true\n";
    assert_eq!(own_returns(inner).len(), 1);
    assert!(own_returns("\n    \"return\" == s\n").is_empty());
    assert!(own_returns("\n    returns_ok()\n").is_empty());
}

#[test]
fn a_return_inside_a_nested_class_method_is_not_the_functions() {
    let body = "\n    class Local {\n        check() {\n            return true;\n        }\n    }\n    return false;\n";
    assert_eq!(own_returns(body), vec![body.rfind("return false").unwrap()]);
}

#[test]
fn a_return_inside_single_quoted_template_or_comment_text_is_ignored() {
    let body = "let message = '🟦 return true;';\nlet template = `return false;`;\n/* return true; */\nreturn result;";
    assert_eq!(
        own_returns(body),
        vec![body.rfind("return result").unwrap()]
    );
}

#[test]
fn a_call_starts_at_its_receiver_or_its_path() {
    let t = "if cfg.limits().is_valid(1) {}";
    assert_eq!(
        call_start(t, t.find("is_valid").unwrap()),
        t.find("cfg").unwrap()
    );
    let t = "let ok = crate::rules::is_valid(1);";
    assert_eq!(
        call_start(t, t.find("is_valid").unwrap()),
        t.find("crate").unwrap()
    );
    let t = "let ok = is_valid(1);";
    assert_eq!(
        call_start(t, t.find("is_valid").unwrap()),
        t.find("is_valid").unwrap()
    );
}

fn report() -> Inverted {
    Inverted {
        was: "is_valid".into(),
        now: "is_invalid".into(),
        root: "/root".into(),
        file: "src/lib.rs".into(),
        kind: "function".into(),
        negated: 2,
        cancelled: 1,
        writes: 0,
        blocked: Vec::new(),
        unmatched: vec!["src/app.rs:9:14 `is_valid` used as a value".into()],
        rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
        diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
        applied: false,
    }
}

#[test]
fn the_report_counts_the_negations_and_names_what_was_left() {
    let text = report().render(10_000);
    assert!(text.contains("`is_valid` → `is_invalid`"), "{text}");
    assert!(
        text.contains("2 call(s) gain a `!`, 1 lose the `!` they had"),
        "{text}"
    );
    assert!(text.contains("used as a value"), "{text}");
    assert!(text.contains("the analyzer rejects the result"), "{text}");
    let mut done = report();
    done.unmatched.clear();
    done.diagnostics.clear();
    done.applied = true;
    let text = done.render(10);
    assert!(
        text.contains("0 errors")
            && text.contains("[applied to 1 file(s)]")
            && text.contains("diff truncated"),
        "{text}"
    );

    let mut field = report();
    field.kind = "field".into();
    field.writes = 3;
    field.blocked = vec!["src/lib.rs:1:1 the struct derives `Default`".into()];
    let text = field.render(10_000);
    assert!(text.contains("a boolean field"), "{text}");
    assert!(
        text.contains("2 read(s) gain a `!`, 1 lose the `!`"),
        "{text}"
    );
    assert!(text.contains("3 write(s) now store the negation"), "{text}");
    assert!(
        text.contains("1 use(s) cannot keep their meaning"),
        "{text}"
    );
}
