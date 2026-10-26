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
fn coverage_b2_match_expression_boundaries() {
    for (expr, expected) in [
        ("f(1, 2), tail", Some(7)),
        ("[1, 2], tail", Some(6)),
        ("{ a(); b() }, tail", Some(12)),
        ("make::<A, B>(), tail", Some(14)),
        ("\"a,b\", tail", Some(5)),
        ("value", None),
        ("a < b, tail", Some(5)),
        ("x /* , */ , tail", Some(10)),
    ] {
        assert_eq!(find_rust_arm_comma(expr), expected, "{expr}");
    }
    let text = "  match kind {\n// note\nA => { one(); }, B => call(1, 2), _ => last() }";
    let result = parse_rust_match(text, text.len()).unwrap();
    assert_eq!(result.start_offset, 2);
    assert_eq!(result.end_offset, text.len());
    assert_eq!(result.indent, "  ");
    assert_eq!(result.discriminator, "kind");
    assert_eq!(
        result
            .branches
            .iter()
            .map(|b| (b.tag.as_str(), b.body.as_str(), b.is_default))
            .collect::<Vec<_>>(),
        vec![
            ("A", "one();", false),
            ("B", "call(1, 2)", false),
            ("_", "last()", true)
        ]
    );
    for text in [
        "",
        "match x",
        "match x {",
        "match x {}",
        "match x { => a }",
        "match x { no arms }",
    ] {
        assert!(parse_rust_match(text, 0).is_none(), "{text}");
    }
}

/// The rolling context before a character is cut by characters: a byte cut inside `é` made
/// `find_rust_arm_comma` panic on an ordinary arm (#1430).
#[test]
fn unicode_identifiers_in_an_arm_do_not_panic() {
    // Byte offsets: `é` is two bytes, each of 日本語 three.
    assert_eq!(find_rust_arm_comma("f(é, 0), tail"), Some(8));
    assert_eq!(find_rust_arm_comma("g(日本語, ü::<A, B>()), x"), Some(26));
    let text = "match k {\n    0 => f(é, 0),\n    _ => 0,\n}";
    let block = parse_rust_match(text, 0).expect("the match parses");
    assert_eq!(
        block
            .branches
            .iter()
            .map(|b| (b.tag.as_str(), b.body.as_str()))
            .collect::<Vec<_>>(),
        vec![("0", "f(é, 0)"), ("_", "0")]
    );
}
