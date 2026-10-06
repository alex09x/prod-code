/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::blank_comments;
use crate::signature::plan::{in_use_or_comment, reorders_to_itself};
use crate::signature::rewrite::{attribute, call_tokens};
use crate::signature::util::line_col_at;

/// A swap of two arguments, as the reconciliation asks of every call.
fn swap(call: &[String], _awaited: bool) -> bool {
    !reorders_to_itself(call, &[Some(1), Some(0)])
}

/// The (line, column) of the `n`th `join` in `text`.
fn join_at(text: &str, n: usize) -> (u32, u32) {
    line_col_at(text, text.match_indices("join").nth(n).expect("there").0).unwrap()
}

#[test]
fn a_reference_is_matched_by_its_own_occurrence_not_its_line() {
    // The value use shares its line with a call that was rewritten; a line-based check
    // took it for rewritten (#446).
    let old = "fn m() {\n    let (f, s) = (join, join(a, b));\n}\n";
    let new = "fn m() {\n    let (f, s) = (join, join(b, a));\n}\n";
    let refs = [join_at(old, 0), join_at(old, 1)];
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs, &swap);
    assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    assert!(
        unmatched[0].starts_with("m.rs:2:19 (not a call"),
        "{unmatched:?}"
    );
    assert!(unexpected.is_empty(), "{unexpected:?}");

    // On lines of their own, the same.
    let old = "let f = join;\nlet s = join(\n    a,\n    b,\n);\n";
    let new = "let f = join;\nlet s = join(b, a);\n";
    let refs = [join_at(old, 0), join_at(old, 1)];
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs, &swap);
    assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    assert!(unmatched[0].starts_with("m.rs:1:9 "), "{unmatched:?}");
    assert!(unexpected.is_empty(), "{unexpected:?}");
}

#[test]
fn a_change_no_reference_explains_is_unexpected() {
    // `other(1, 2)` on the line of a listed call was rewritten too.
    let old = "let s = join(a, b) + other(1, 2);\nlet t = join(c, d);\n";
    let new = "let s = join(b, a) + other(2, 1);\nlet t = join(d, c);\n";
    let refs = [join_at(old, 0), join_at(old, 1)];
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs, &swap);
    assert_eq!(unexpected, ["m.rs:1"]);
    // What comes after it is not taken as rewritten.
    assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    assert!(unmatched[0].starts_with("m.rs:2:9 (after a change"));

    // A call the analyzer did not list is a change no reference explains.
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs[..1], &swap);
    assert!(unmatched.is_empty(), "{unmatched:?}");
    assert_eq!(unexpected, ["m.rs:1"]);
}

#[test]
fn calls_left_as_they_were_stale_positions_imports_and_comments_are_told_apart() {
    let old =
        "use crate::join;\n// see join\nlet s = join(a, b);\nlet t = join(c, c);\nlet u = x;\n";
    let new =
        "use crate::join;\n// see join\nlet s = join(a, b);\nlet t = join(c, c);\nlet u = x;\n";
    let refs = [
        join_at(old, 0),
        join_at(old, 1),
        join_at(old, 2),
        join_at(old, 3),
        (5, 9),
        (9, 1),
    ];
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs, &swap);
    assert!(unexpected.is_empty(), "{unexpected:?}");
    // The import and the comment name it and still do; `join(c, c)` reorders to itself.
    assert_eq!(
        unmatched,
        [
            "m.rs:5:9 (the analyzer places `join` here, but the file says otherwise)",
            "m.rs:9:1 (no such position in the file)",
            "m.rs:3:9 (a call the rewrite left as it was)",
        ]
    );
}

#[test]
fn a_respelled_callee_a_method_chain_and_a_nested_call_are_attributed() {
    // A rewrite may respell the path to the function and drop the whitespace before a
    // method's `.`; a call in another's arguments is rewritten inside it.
    let old = "let s = m::join(a, b);\nlet t = v\n    .join(c, d)\n    .join(e, f);\nlet u = join(join(1, 2), 3);\n";
    let new =
        "let s = join(b, a);\nlet t = v.join(d, c).join(f, e);\nlet u = join(3, join(2, 1));\n";
    let refs: Vec<(u32, u32)> = (0..5).map(|n| join_at(old, n)).collect();
    let (unmatched, unexpected) = attribute("m.rs", old, new, "join", &refs, &swap);
    assert!(unmatched.is_empty(), "{unmatched:?}");
    assert!(unexpected.is_empty(), "{unexpected:?}");

    // The inner call left in the old order is named.
    let lazy =
        "let s = join(b, a);\nlet t = v.join(d, c).join(f, e);\nlet u = join(3, join(1, 2));\n";
    let (unmatched, _) = attribute("m.rs", old, lazy, "join", &refs, &swap);
    assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    assert!(unmatched[0].starts_with("m.rs:5:14 (a call inside another"));
}

#[test]
fn an_await_is_part_of_the_call_it_follows() {
    let old = "async fn m() {\n    let f = load;\n    load(1);\n}\n";
    let new = "async fn m() {\n    let f = load;\n    load(1).await;\n}\n";
    let at = |n: usize| line_col_at(old, old.match_indices("load").nth(n).unwrap().0).unwrap();
    let awaits = |_: &[String], awaited: bool| !awaited;
    let (unmatched, unexpected) = attribute("m.rs", old, new, "load", &[at(0), at(1)], &awaits);
    assert!(unexpected.is_empty(), "{unexpected:?}");
    assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    assert!(unmatched[0].starts_with("m.rs:2:13 (not a call"));
}

mod raw_identifier_import_tests {
    use super::*;

    #[test]
    fn raw_use_is_a_value_and_raw_module_imports_are_imports() {
        for (text, import) in [
            ("fn caller() { take(r#use, callee); }", false),
            ("fn caller() { let x = Holder { r#use: callee }; }", false),
            ("use r#type::{first, callee};", true),
            ("use /* note */ crate::{first, callee};", true),
            ("use other::first; fn caller() { take(callee); }", false),
        ] {
            let at = text.find("callee").unwrap();
            let code = blank_comments(text);
            assert_eq!(
                in_use_or_comment(text, code.as_deref(), at),
                import,
                "{text}"
            );
        }
    }

    #[test]
    fn raw_use_does_not_hide_an_unmatched_function_value() {
        let old = "fn caller() { take(r#use, callee); callee(1, 2); }";
        let new = old.replace("callee(1, 2)", "callee(2, 1)");
        let refs = old
            .match_indices("callee")
            .map(|(i, _)| (1, i as u32 + 1))
            .collect::<Vec<_>>();
        let (unmatched, unexpected) =
            attribute("src/lib.rs", old, &new, "callee", &refs, &|_, _| true);
        assert_eq!(unmatched.len(), 1, "{unmatched:?}");
        assert!(unmatched[0].contains("not a call"), "{unmatched:?}");
        assert!(unexpected.is_empty(), "{unexpected:?}");
    }
}

mod argument_spelling_tests {
    use super::*;

    #[test]
    fn spaces_inside_string_literals_are_values() {
        let args = vec![r#""a  b""#.to_string(), r#""a b""#.to_string()];
        assert!(!reorders_to_itself(&args, &[Some(1), Some(0)]));
    }
}

mod relocated_comment_tests {
    use super::*;

    fn compare(old: &str, new: &str) -> (Vec<String>, Vec<String>) {
        let at = old.find("f(").unwrap();
        attribute("a.rs", old, new, "f", &[(1, at as u32 + 1)], &|_, _| true)
    }

    #[test]
    fn unchanged_nested_block_comments_can_follow_a_rewritten_call() {
        for (old, new) in [
            (
                "fn c() { f(3 /* p, /* then */ q, */, 4); }",
                "fn c() { f(4, 3)/* p, /* then */ q, */; }",
            ),
            (
                "fn c() { f(1 /* x */, 2 /* x */); }",
                "fn c() { f(2, 1) /* x */ /* x */; }",
            ),
            (
                "fn c() { f(1 /* a */, 2 /* b */); }",
                "fn c() { f(2 /* b */, 1) /* a */; }",
            ),
            (
                "fn c() { f(1 /* x */, 2).await; }",
                "fn c() { f(2, 1).await/* x */; }",
            ),
        ] {
            assert_eq!(compare(old, new), (vec![], vec![]), "{old} -> {new}");
        }
    }

    #[test]
    fn lost_changed_or_invented_comments_and_unrelated_changes_still_refuse() {
        let old = "fn c() { f(1 /* x */, 2); keep(); }";
        for new in [
            "fn c() { f(2, 1); keep(); }",
            "fn c() { f(2, 1)/* y */; keep(); }",
            "fn c() { f(2, 1)/* x *//* x */; keep(); }",
            "fn c() { f(2, 1)/* x */; changed(); }",
            "fn c() { f(2 /* invented */, 1)/* x */; keep(); }",
        ] {
            assert!(!compare(old, new).1.is_empty(), "{new}");
        }
        for (old, new) in [
            (
                "fn c() { f(1 /* x */, 2)/* x */; }",
                "fn c() { f(2, 1)/* x */; }",
            ),
            (
                "fn c() { f(1 /** doc */, 2); }",
                "fn c() { f(2, 1)/** doc */; }",
            ),
            ("fn c() { f(1 // x\n, 2); }", "fn c() { f(2, 1)// x\n; }"),
        ] {
            assert!(!compare(old, new).1.is_empty(), "{old} -> {new}");
        }
    }

    #[test]
    fn moving_only_a_comment_does_not_count_as_rewriting_a_call() {
        let (unmatched, unexpected) =
            compare("fn c() { f(1 /* x */, 2); }", "fn c() { f(1, 2)/* x */; }");
        assert_eq!(unmatched.len(), 1);
        assert!(unexpected.is_empty());
        let (comments, code) =
            call_tokens(r##"(r#"/* text */ a b"#, "x y", 1 /* real */)"##).unwrap();
        assert_eq!(comments, vec!["/* real */"]);
        assert_eq!(code, br##"(r#"/* text */ a b"#,"x y",1)"##);
    }
}
