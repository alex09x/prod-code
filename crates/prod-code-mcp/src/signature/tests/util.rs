/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::Path;

use crate::signature::util::{
    display, line_col_at, normalize, normalize_type_spelling, position_at, read_caller,
    typed_locals_before,
};

#[test]
fn a_caller_that_cannot_be_read_stops_the_change() {
    let dir = tempfile::Builder::new().prefix("su").tempdir().unwrap();
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    assert_eq!(read_caller(&file).unwrap(), "fn a() {}\n");
    let missing = dir.path().join("gone.rs");
    assert_eq!(
        read_caller(&missing).unwrap_err().to_string(),
        format!("cannot read {}; nothing was written", missing.display())
    );
    assert_eq!(display(dir.path(), &file), "a.rs");
    assert_eq!(display(Path::new("/x"), Path::new("/y/b.rs")), "/y/b.rs");
}

#[test]
fn a_parameter_list_is_reported_on_one_line_without_its_trailing_comma() {
    let cases = [
        ("a: i32,\n    b: u8,\n", "a: i32, b: u8"),
        ("", ""),
        ("  x  ", "x"),
        ("a,\tb", "a, b"),
    ];
    for (list, want) in cases {
        assert_eq!(normalize(list), want, "{list:?}");
    }
}

#[test]
fn offsets_off_a_position_are_refused() {
    assert_eq!(line_col_at("a\nbc", 3), Some((2, 2)));
    assert_eq!(line_col_at("", 0), Some((1, 1)));
    assert_eq!(line_col_at("é", 1), None, "inside a character");
    assert_eq!(line_col_at("ab", 3), None, "past the end");
    assert_eq!(line_col_at("a\r\nb", 2), None, "between \\r and \\n");
    assert_eq!(position_at("a\nbc", 3).unwrap(), (2, 2));
    assert_eq!(
        position_at("é", 1).unwrap_err().to_string(),
        "byte 1 of a 2-byte file is on no line and column; nothing was planned"
    );
}

#[test]
fn type_spelling_drops_lifetimes_and_keeps_only_the_spaces_that_separate_words() {
    let cases = [
        ("&'a mut Vec<T>", "&mut Vec<T>"),
        ("&'static str", "&str"),
        ("  Vec < u8 >  ", "Vec<u8>"),
        ("dyn Fn(u8)  ->  u8", "dyn Fn(u8)->u8"),
        ("'a'", "'a'"),
        ("x'", "x'"),
        ("&'a\n  T", "&T"),
        ("", ""),
    ];
    for (ty, want) in cases {
        assert_eq!(normalize_type_spelling(ty), want, "{ty:?}");
    }
}

fn locals(body: &str) -> BTreeMap<String, String> {
    let text = format!("fn f() {{\n{body}\nHERE\n}}");
    typed_locals_before(&text, text.find("HERE").unwrap())
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn typed_lets_before_a_call_are_collected_and_shadows_remove_them() {
    let cases: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "let a: &'a str = s;\nlet b = 2;\nlet mut c : Vec<u8> = v;",
            map(&[("a", "&str"), ("c", "Vec<u8>")]),
        ),
        ("let a: i32;", map(&[("a", "i32")])),
        ("let a: i32 = 1;\nlet a = 2;", map(&[])),
        ("let a: i32 = 1\n", map(&[])),
        ("let a: i32 = 1;\n{ let a: u8 = 2; }", map(&[("a", "i32")])),
        ("let a: i32 = 1;\nv.iter().map(|a| a);", map(&[])),
        ("let a: i32 = 1;\nlet t = x || y;", map(&[("a", "i32")])),
        (
            "let a: i32 = 1;\nmatch x { Some(a) => a, _ => 0 };",
            map(&[("a", "i32")]),
        ),
        (
            "let a: i32 = 1;\nif let Some(a) = o { }",
            map(&[("a", "i32")]),
        ),
        ("let a: i32 = 1;\nfor a in 0..3 { }", map(&[("a", "i32")])),
        ("let (p, q): (u8, u8) = t;", map(&[])),
        ("let r: &'a str = \"x;y\";", map(&[("r", "&str")])),
        ("// let z: u8 = 1;\nlet w: u8 = 2;", map(&[("w", "u8")])),
        ("let v: i32 = if c { 1 } else { 2 };", map(&[("v", "i32")])),
        ("let f: fn(u8) -> u8 = g;", map(&[("f", "fn(u8)->u8")])),
        ("let a: i32 = 1;\nfn g(a: u8) {}", map(&[("a", "i32")])),
        ("while let Some(a) = it.next() { }", map(&[])),
        ("let b: [u8; 3] = [1, 2, 3];", map(&[("b", "[u8;3]")])),
        ("let t: (u8, u8) = (1, 2);", map(&[("t", "(u8,u8)")])),
        (
            "let c: bool = a == b;\nlet d: bool = a >= b;",
            map(&[("c", "bool"), ("d", "bool")]),
        ),
        ("let a: i32 = 1;\nlet Some(a): Option<i32> = o;", map(&[])),
        ("x |= 1;\nlet a: u8 = 2;", map(&[("a", "u8")])),
    ];
    for (body, want) in cases {
        assert_eq!(locals(body), want, "{body:?}");
    }
}

#[test]
fn a_binding_shadowed_by_a_later_for_loop_or_fn_is_not_reported_from_inside_it() {
    let text = "fn f() {\nlet a: i32 = 1;\nfor a in 0..3 {\nHERE\n}\n}";
    assert_eq!(
        typed_locals_before(text, text.find("HERE").unwrap()),
        map(&[])
    );
    let text = "let a: i32 = 1;\nHERE";
    assert_eq!(
        typed_locals_before(text, text.find("HERE").unwrap()),
        map(&[("a", "i32")])
    );
}
