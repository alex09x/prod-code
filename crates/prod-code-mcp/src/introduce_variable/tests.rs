/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::analysis::{
    can_panic, changes, enclosing_body, innermost_block, loops_after, occurrences, parenthesised,
    reads_and_effects, statement_start, surely_evaluated,
};

#[test]
fn an_expression_that_calls_or_awaits_is_not_one_value() {
    assert_eq!(reads_and_effects("(w + 1)"), (vec!["w".to_string()], None));
    assert_eq!(
        reads_and_effects("a.b * LIMIT as u64").0,
        vec!["a".to_string()]
    );
    for with_effect in ["f(x)", "v.len()", "format!(\"x\")", "r?", "fut.await"] {
        assert!(reads_and_effects(with_effect).1.is_some(), "{with_effect}");
    }
}

#[test]
fn occurrences_are_whole_and_a_changed_name_is_seen() {
    let t = "let a = (w + 1) * 2; let b = (w + 1) + 3; let c = x(w + 1);";
    assert_eq!(occurrences(t, 0, t.len(), "(w + 1)").len(), 2);
    let f = "fn outer(w: u32) -> u32 {\n    fn inner() {}\n    w + 1\n}\n";
    let (open, close) = enclosing_body(f, f.find("w + 1").unwrap()).unwrap();
    assert_eq!((&f[open..open + 1], &f[close..close + 1]), ("{", "}"));
    assert!(f[open..close].contains("fn inner"));
    assert!(enclosing_body(f, 3).is_none());
    assert_eq!(occurrences("aw + 1 + w + 1", 0, 14, "w + 1").len(), 1);
    assert!(changes("w += 1;", "w"));
    assert!(changes("let w = 3;", "w"));
    assert!(changes("f(&mut w);", "w"));
    assert!(changes("w.inner.len = 2;", "w"));
    assert!(!changes("let n = w.len == 2;", "w"));
    assert!(!changes("let x = w == 1;", "w"));
    assert!(!changes("let ww = 1; www = 2;", "w"));
}

#[test]
fn the_binding_goes_before_the_statement_in_the_block_that_holds_them_all() {
    let f = "fn f(w: u32, h: u32) -> u32 {\n    let x = 1;\n    let a = if h > 2 {\n        (w + 1) * h\n    } else {\n        0\n    };\n    a + (w + 1)\n}\n";
    let found = occurrences(f, 0, f.len(), "(w + 1)");
    let body = f.find('{').unwrap();
    assert_eq!(innermost_block(f, body, found[0], found[1]), body);
    let at = statement_start(f, body, found[0]);
    assert!(f[at..].starts_with("let a = if"), "{}", &f[at..]);
    let g = "fn g() { if c { let y = 2; foo(w + 1, w + 1) } }";
    let found = occurrences(g, 0, g.len(), "w + 1");
    let inner = innermost_block(g, g.find('{').unwrap(), found[0], found[1]);
    assert_eq!(&g[inner - 2..inner], "c ");
    assert!(g[statement_start(g, inner, found[0])..].starts_with("foo("));
    // The `}` before `else` does not end the statement.
    let e = "fn e() { let a = if c { 1 } else { w + 1 }; a + (w + 1) }";
    let at = statement_start(e, e.find('{').unwrap(), e.find("w + 1").unwrap());
    assert!(e[at..].starts_with("let a"), "{}", &e[at..]);
}

#[test]
fn a_method_call_on_a_name_may_change_it() {
    assert!(changes("s.bump();", "s"));
    assert!(changes("s.inner.push(1);", "s"));
    assert!(!changes("let y = s.x;", "s"));
}

#[test]
fn a_guarded_division_is_not_surely_evaluated() {
    assert!(can_panic("a / b") && can_panic("v[i]") && can_panic("w + 1"));
    assert!(!can_panic("s.x") && !can_panic("flag"));
    let r = "fn r() {\n    let mut r = 0;\n    if b != 0 {\n        r += a / b;\n    }\n    r + a / b\n}";
    let found = occurrences(r, 0, r.len(), "a / b");
    let body = r.find('{').unwrap();
    let anchor = statement_start(r, body, found[0]);
    assert!(r[anchor..].starts_with("if b != 0"));
    // Inside the `if`: not every time. After it, at the block's level: every time.
    let own = |at: usize| statement_start(r, innermost_block(r, body, at, at), at);
    assert!(!surely_evaluated(r, anchor, own(found[0]), found[0]));
    assert!(surely_evaluated(r, anchor, own(found[1]), found[1]));
    // A way out before it, or a short circuit in its own statement, and it is not.
    let early = "fn e() {\n    if c { return 0; }\n    a / b\n}";
    let at = early.find("a / b").unwrap();
    assert!(!surely_evaluated(early, early.find("if").unwrap(), at, at));
    let short = "fn s() {\n    let z = b != 0 && a / b > 1;\n}";
    let at = short.find("a / b").unwrap();
    let st = short.find("let z").unwrap();
    assert!(!surely_evaluated(short, st, st, at));
}

#[test]
fn an_occurrence_in_parentheses_of_its_own_loses_them() {
    let t = "a + (s.x + 1) + f(s.x + 1) + v[(s.x + 1)]";
    let spans: Vec<_> = occurrences(t, 0, t.len(), "s.x + 1")
        .into_iter()
        .map(|at| &t[parenthesised(t, at, at + 7)])
        .collect();
    assert_eq!(spans, ["(s.x + 1)", "s.x + 1", "(s.x + 1)"]);
}

#[test]
fn a_loop_after_the_first_occurrence_is_watched_whole() {
    let t = "let a = w + 1; while go { f(w + 1); w += 1; } for_each(x); loops;";
    let found = occurrences(t, 0, t.len(), "w + 1");
    let loops = loops_after(t, found[0], found[1]);
    assert_eq!(loops.len(), 1);
    let (open, close) = loops[0];
    assert!(t[open..=close].contains("w += 1"));
    assert!(changes(&t[open..close], "w"));
    assert!(!changes(&t[found[0]..found[1] + 5], "w"));
}
