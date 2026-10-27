/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Duplicate safety of Rust extraction (#1438): copies in comments and raw strings, bindings
//! with an uppercase name, and later text that only spells a name.

use super::{bound_names, read_after_but_not_returned};
use crate::extract_function::duplicates::copies_of;

/// Code in a block comment or a raw string is no copy of the selection.
#[test]
fn copies_are_not_found_in_block_comments_or_raw_strings() {
    let text = "fn f(x: u32) -> u32 {\n    let a = x + 1;\n    /* x + 1 */\n    let s = r#\"prefix \" x + 1 \" suffix\"#;\n    let b = br\"x + 1\";\n    let c = x + 1;\n    a + c\n}\n";
    let s = text.find("x + 1").unwrap();
    let e = s + "x + 1".len();
    let found = copies_of(&text[s..e], text, Some((s, e)), false);
    let at: Vec<&str> = found.iter().map(|c| &text[c.from - 8..c.to]).collect();
    assert_eq!(found.len(), 1, "{at:?}");
    assert_eq!(
        found[0].from,
        text.find("let c = x + 1").unwrap() + "let c = ".len()
    );
}

/// A binding is told by its pattern role, not its case.
#[test]
fn uppercase_locals_are_bound_and_constructors_are_not() {
    assert_eq!(
        bound_names(
            "let Gross = a; let Some(n) = o else { return }; let P { x } = p; let E::V(y) = q; let None = r else { return };"
        ),
        vec!["Gross", "n", "x", "y"]
    );
    let selection = "let Gross = o.qty * o.price;\n    let net = Gross - 1;";
    let call = "let net = net_price(o);";
    let text = "fn s(o: &O) -> u32 {\n    let Gross = o.qty * o.price;\n    let net = Gross - 1;\n    Gross + net\n}\n";
    let to = text.find("Gross - 1;").unwrap() + "Gross - 1;".len();
    assert_eq!(
        read_after_but_not_returned(text, selection, call, to).as_deref(),
        Some("Gross")
    );
}

/// Fields, strings, comments and a later shadowing `let` only spell the name: they do not read
/// the selection's binding. Its initializer, and an `if let` body, still do.
#[test]
fn only_real_reads_after_the_place_block_a_duplicate() {
    let selection = "let gross = o.qty * o.price;\n    let net = gross - 1;";
    let call = "let net = net_price(o);";
    let read_after = |tail: &str| {
        let text = format!(
            "fn s(o: &O) -> u32 {{\n    let gross = o.qty * o.price;\n    let net = gross - 1;\n{tail}\n}}\n"
        );
        let to = text.find("gross - 1;").unwrap() + "gross - 1;".len();
        read_after_but_not_returned(&text, selection, call, to)
    };
    for harmless in [
        "    let t = o.gross + order.gross();\n    net + t",
        "    log(\"gross\"); // gross\n    /* gross */ net",
        "    let p = Price { gross: net };\n    p.gross",
        "    let gross = 7;\n    gross + net",
        "    let gross;\n    gross = 7;\n    gross + net",
        "    let x = Kind::gross;\n    net",
    ] {
        assert_eq!(read_after(harmless), None, "{harmless}");
    }
    for read in [
        "    net + gross",
        "    let gross = gross + 1;\n    gross + net",
        "    if let Some(gross) = o.next() { gross } else { net }",
        "    { let gross = 1; }\n    gross + net",
        "    let p = Price { gross };\n    p.total",
        "    (0..gross).count() as u32 + net",
    ] {
        assert_eq!(read_after(read).as_deref(), Some("gross"), "{read}");
    }
}
