/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::binding::{bound_names, read_after_but_not_returned};
use super::duplicates::copies_of;
use super::rewrite::{apply_edits, literal_type, rename_placeholder, rewrite_of, with_arguments};
use super::tokens::{Token, mentions, tokens};
use super::types::PLACEHOLDER;

const THREE: &str = "pub fn invoice(o: &Order) -> u32 {\n    let gross = o.qty * o.price;\n    let net = gross - gross * o.discount / 100;\n    net + 5\n}\n\npub fn quote(o: &Order) -> u32 {\n    let gross = o.qty * o.price;\n        let net = gross - gross *  o.discount / 100;\n    net\n}\n\npub fn other(o: &Order) -> u32 {\n    let grossly = o.qty * o.price;\n    ungross(o)\n}\n";

#[test]
fn a_name_the_call_does_not_return_is_not_read_after_a_duplicate() {
    assert_eq!(
        bound_names(
            "let gross = a; let mut n: u32 = 1; let (x, _y) = p; let P { f, g: h } = q; let _ = z;"
        ),
        vec!["_y", "f", "gross", "h", "n", "x"]
    );
    let selection = "let gross = o.qty * o.price;\n    let net = gross - 1;";
    let call = "let net = net_price(o);";
    let shadowed = "fn s(o: &O) -> u32 {\n    let gross = 1000;\n    let gross = o.qty * o.price;\n    let net = gross - 1;\n    gross - net\n}\n";
    let to = shadowed.find("gross - 1;").unwrap() + "gross - 1;".len();
    assert_eq!(
        read_after_but_not_returned(shadowed, selection, call, to).as_deref(),
        Some("gross")
    );
    let fine = "fn q(o: &O) -> u32 {\n    let gross = o.qty * o.price;\n    let net = gross - 1;\n    net\n}\nfn later() { gross(); }\n";
    let to = fine.find("gross - 1;").unwrap() + "gross - 1;".len();
    assert_eq!(read_after_but_not_returned(fine, selection, call, to), None);
}

#[test]
fn copies_are_found_token_for_token_and_only_whole() {
    let start = THREE.find("let gross").unwrap();
    let end = THREE.find("/ 100;").unwrap() + "/ 100;".len();
    let found = copies_of(&THREE[start..end], THREE, Some((start, end)), false);
    assert_eq!(found.len(), 1, "{found:?}");
    let c = &found[0];
    assert!(THREE[c.from..c.to].starts_with("let gross"));
    assert!(THREE[c.from..c.to].ends_with("/ 100;"));
    assert!(c.from > end && c.differs.is_empty());
    // An expression glued to a longer name is not the same code.
    let g = "let a = gross + 1; let b = ungross + 1; let c = (gross + 1);";
    let s = g.find("gross + 1").unwrap();
    let e = s + "gross + 1".len();
    let found = copies_of(&g[s..e], g, Some((s, e)), false);
    assert_eq!(found.len(), 1);
    assert_eq!(&g[found[0].from..found[0].to], "gross + 1");
    assert!(found[0].from > g.find("ungross").unwrap());
    // Statements must start a statement: `a.y = 1;` holds `y = 1;` and is not it.
    let h = "fn h() { y = 1; a.y = 1; if c { y = 1; } }";
    let s = h.find("y = 1;").unwrap();
    let found = copies_of("y = 1;", h, Some((s, s + 6)), false);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(h[..found[0].from].ends_with("{ "));
    assert!(copies_of("", "fn e() {}", None, false).is_empty());
}

#[test]
fn a_copy_may_differ_in_literals_of_the_same_kind_only_when_asked() {
    let text = "a; x / 50; y / 100; x / \"s\"; x / 7;";
    let exact = copies_of("x / 100;", text, None, false);
    assert!(exact.is_empty(), "{exact:?}");
    let near = copies_of("x / 100;", text, None, true);
    let differs: Vec<_> = near.iter().map(|c| c.differs.clone()).collect();
    assert_eq!(
        differs,
        vec![vec![(2, "50".to_string())], vec![(2, "7".to_string())]]
    );
}

#[test]
fn tokens_tell_literals_names_and_lifetimes_apart() {
    let text = "let s: &'a str = \"a \\\" b\"; let c = 'x'; for i in 1..3 { f(2.5u8) } // done";
    let kinds: Vec<(Token, &str)> = tokens(text)
        .iter()
        .map(|(k, s, e)| (*k, &text[*s..*e]))
        .collect();
    assert!(kinds.contains(&(Token::Word, "'a")), "{kinds:?}");
    assert!(kinds.contains(&(Token::Str, "\"a \\\" b\"")), "{kinds:?}");
    assert!(kinds.contains(&(Token::Char, "'x'")), "{kinds:?}");
    assert!(kinds.contains(&(Token::Number, "1")) && kinds.contains(&(Token::Number, "3")));
    assert!(kinds.contains(&(Token::Number, "2.5u8")), "{kinds:?}");
    assert!(
        !kinds.iter().any(|(_, s)| *s == "done"),
        "comments are skipped"
    );
}

#[test]
fn a_literal_s_type_comes_from_the_hover_and_calls_gain_arguments() {
    assert_eq!(
        literal_type("```rust\nu32\n```\n---\n\nvalue of literal: ` 100 `").as_deref(),
        Some("u32")
    );
    assert_eq!(literal_type("```rust\n&str\n```").as_deref(), Some("&str"));
    assert_eq!(
        literal_type("```rust\nfn div(self, other: u32) -> u32\n```"),
        None
    );
    assert_eq!(literal_type("no code"), None);
    assert_eq!(
        with_arguments("let n = fun_name(o);", &["100".into()]).as_deref(),
        Some("let n = fun_name(o, 100);")
    );
    assert_eq!(
        with_arguments("fun_name()", &["1".into(), "2".into()]).as_deref(),
        Some("fun_name(1, 2)")
    );
    assert_eq!(with_arguments("x", &[]).as_deref(), Some("x"));
    assert_eq!(with_arguments("no call", &["1".into()]), None);
    let edited = apply_edits("abcdef", &[(1, 2, "X".into()), (4, 4, "Y".into())]);
    assert_eq!(edited, "aXcdYef");
}

#[test]
fn the_rewrite_is_read_even_when_the_body_repeats_the_selection() {
    let old = "fn f(o: &O) -> u32 {\n    let g = o.a;\n    let n = g + 1;\n    n\n}\n\nfn z() {}\n";
    let new = "fn f(o: &O) -> u32 {\n    let n = fun_name(o);\n    n\n}\n\nfn fun_name(o: &O) -> u32 {\n    let g = o.a;\n    let n = g + 1;\n    n\n}\n\nfn z() {}\n";
    let start = old.find("let g").unwrap();
    let end = old.find("+ 1;").unwrap() + 4;
    let rewrite = rewrite_of(old, new, start, end).expect("a plain extraction");
    assert_eq!(rewrite.call, "let n = fun_name(o);");
    // Text after the selection and after the new function lands where it is in `new`.
    let n_at = old.find("    n\n}").unwrap();
    let mapped = rewrite.mapped(start, end, n_at, n_at + 5).unwrap();
    assert_eq!(&new[mapped..mapped + 5], "    n");
    let z = old.find("fn z").unwrap();
    let mapped = rewrite.mapped(start, end, z, z + 4).unwrap();
    assert_eq!(&new[mapped..mapped + 4], "fn z");
    // Inside the selection nothing maps; before it everything does, as it was.
    assert_eq!(rewrite.mapped(start, end, start, end), None);
    assert_eq!(rewrite.mapped(start, end, 0, 2), Some(0));
    // A change before the selection: the call does not stand on its own.
    let moved = new.replacen("fn f(o", "fn f(mut o", 1);
    assert_eq!(rewrite_of(old, &moved, start, end), None);
    // No new function, or one before the selection: not an extraction this can read.
    assert_eq!(rewrite_of(old, old, start, end), None);
}

#[test]
fn the_placeholder_is_renamed_only_whole() {
    assert_eq!(
        rename_placeholder("fun_name(x) + fun_names", "f"),
        "f(x) + fun_names"
    );
    assert!(mentions("a fun_name b", PLACEHOLDER) && !mentions("fun_names", PLACEHOLDER));
}
