/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lsp::hover_type;
use super::syntax::{
    braces_are_pattern, derives_above, expression_end, inside_string, is_struct_brace, negation_of,
};
use super::types::{ValueKind, value_kind};

fn kind_of(text: &str, name: &str) -> Option<ValueKind> {
    value_kind(text, text.find(name).unwrap(), name)
}

#[test]
fn a_bool_field_and_a_let_are_recognised_and_nothing_else() {
    let s = "#[derive(Debug)]\npub struct S {\n    pub enabled: bool,\n    pub n: u32,\n}\n";
    assert!(matches!(
        kind_of(s, "enabled"),
        Some(ValueKind::Field { .. })
    ));
    assert_eq!(kind_of(s, "n: u32"), None);
    assert_eq!(
        kind_of("fn f() { let flag: bool = true; }", "flag"),
        Some(ValueKind::Local { annotated: true })
    );
    assert_eq!(
        kind_of("fn f() { let mut flag = g(); }", "flag"),
        Some(ValueKind::Local { annotated: false })
    );
    assert_eq!(kind_of("fn f(outlet: bool) {}", "outlet"), None);
    assert_eq!(kind_of("fn f(x: bool) {}", "x"), None);
}

#[test]
fn a_negation_cancels_and_a_literal_flips() {
    assert_eq!(negation_of(" v"), "!(v)");
    assert_eq!(negation_of(" !v"), "v");
    assert_eq!(negation_of("!x.ready()"), "x.ready()");
    assert_eq!(negation_of(" a && b"), "!(a && b)");
    assert_eq!(negation_of(" true"), "false");
    assert_eq!(negation_of("false"), "true");
    assert_eq!(negation_of("!a || b"), "!(!a || b)");
}

#[test]
fn derives_and_patterns_and_strings_are_seen() {
    let s = "#[derive(Default, serde::Serialize)]\n/// doc\npub struct S {\n    x: bool,\n}\n";
    let header = s.find("pub struct").unwrap();
    assert_eq!(derives_above(s, header), ["Default", "Serialize"]);
    let p = "let S { x, .. } = s;\nlet t = S { x };\n";
    assert!(braces_are_pattern(p, p.find('{').unwrap()));
    assert!(!braces_are_pattern(p, p.rfind('{').unwrap()));
    let b = "if flag { x } else { y }; Flags { x }; Self { x }; m::Flags { x }";
    let opens: Vec<usize> = b.match_indices('{').map(|(i, _)| i).collect();
    let kinds: Vec<bool> = opens.iter().map(|&o| is_struct_brace(b, o)).collect();
    assert_eq!(kinds, [false, false, true, true, true]);
    let q = "println!(\"{x}\"); x";
    assert!(inside_string(q, q.find("x}").unwrap()));
    assert!(!inside_string(q, q.rfind('x').unwrap()));
    assert_eq!(expression_end("a = f(1, 2), b", 4), 11);
    assert_eq!(expression_end("x = y && z;", 4), 10);
    assert_eq!(
        hover_type("```rust\nlet flag: bool\n```").as_deref(),
        Some("bool")
    );
    assert_eq!(
        hover_type("```rust\nlet n: u32\n```").as_deref(),
        Some("u32")
    );
}
