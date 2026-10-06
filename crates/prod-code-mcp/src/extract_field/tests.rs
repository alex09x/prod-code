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
fn the_type_an_impl_is_for_is_the_last_segment_of_its_path() {
    assert_eq!(self_type(" Store ").as_deref(), Some("Store"));
    assert_eq!(
        self_type("<T> Page<T> where T: Clone ").as_deref(),
        Some("Page")
    );
    assert_eq!(
        self_type("<'a> fmt::Display for crate::store::Store<'a> ").as_deref(),
        Some("Store")
    );
    assert_eq!(
        self_type(" Trait for &mut Store ").as_deref(),
        Some("Store")
    );
    assert_eq!(self_type("  ").as_deref(), None);
}

const SOURCE: &str = "pub struct Store {\n    entries: Vec<u32>,\n}\n\nimpl Store {\n    pub fn new() -> Self {\n        Self { entries: Vec::new() }\n    }\n\n    // don't count this: fn fake() {}\n    pub fn limit(&self) -> usize {\n        let cap = 64 * 1024;\n        cap.min(self.entries.len())\n    }\n}\n\nimpl Default for Store {\n    fn default() -> Store {\n        Store::new()\n    }\n}\n";

#[test]
fn impl_blocks_and_the_method_around_a_position_are_found_by_their_braces() {
    let blocks = impl_blocks(SOURCE);
    assert_eq!(blocks.len(), 2);
    assert!(blocks.iter().all(|(ty, ..)| ty == "Store"));
    let (_, _, open, close) = blocks[0];
    let at = SOURCE.find("64 * 1024").unwrap();
    let (name, params, body_open, body_close) = method_at(SOURCE, open, close, at).unwrap();
    assert_eq!(name, "limit");
    assert_eq!(params, "&self");
    assert!(body_open < at && at < body_close);
    assert!(method_at(SOURCE, open, close, open + 1).is_none());
    assert!(impl_blocks("fn implement() {}\nimpl Missing;\n").is_empty());
}

#[test]
fn a_new_field_goes_last_and_keeps_the_shape_of_the_list() {
    let open = SOURCE.find('{').unwrap();
    let close = crate::parameter_object::matching_bracket(SOURCE, open).unwrap();
    let (at, len, text) = field_insertion(SOURCE, open, close, "cap: usize");
    let mut out = SOURCE.to_string();
    out.replace_range(at..at + len, &text);
    assert!(
        out.starts_with("pub struct Store {\n    entries: Vec<u32>,\n    cap: usize,\n}\n"),
        "{out}"
    );

    let bare = "struct A {\n    a: u8\n}";
    let (at, len, text) = field_insertion(bare, 9, bare.len() - 1, "b: u8");
    let mut out = bare.to_string();
    out.replace_range(at..at + len, &text);
    assert_eq!(out, "struct A {\n    a: u8,\n    b: u8,\n}");

    let empty = "struct A {}";
    let (at, len, text) = field_insertion(empty, 9, 10, "b: u8");
    let mut out = empty.to_string();
    out.replace_range(at..at + len, &text);
    assert_eq!(out, "struct A {\n    b: u8,\n}");
}

fn kind(text: &str) -> Braces {
    let open = text.find('{').unwrap();
    let close = crate::parameter_object::matching_bracket(text, open).unwrap();
    braces_kind(text, open, close)
}

#[test]
fn braces_are_a_pattern_when_something_is_matched_against_them() {
    assert_eq!(kind("let s = Store { entries };"), Braces::Literal);
    assert_eq!(kind("f(Store { entries }, 1)"), Braces::Literal);
    assert_eq!(kind("x == Store { entries }"), Braces::Literal);
    assert_eq!(
        kind("let Store { entries } = s;"),
        Braces::Pattern { rest: false }
    );
    assert_eq!(
        kind("Store { entries, .. } => 1,"),
        Braces::Pattern { rest: true }
    );
    assert_eq!(
        kind("Store { .. } | Other => 1,"),
        Braces::Pattern { rest: true }
    );
    assert_eq!(
        kind("fn f(Store { entries }: Store) {}"),
        Braces::Pattern { rest: false }
    );
    for (text, rest) in [
        ("for Store { entries } in all {}", false),
        ("Store { entries } if entries.is_empty() => 1,", false),
        ("Some(Store { entries }) => 1,", false),
        ("(Store { entries, .. }, 2) => 1,", true),
        ("let [Store { entries }] = all;", false),
        ("assert!(matches!(s, Store { entries, .. }));", true),
    ] {
        assert_eq!(kind(text), Braces::Pattern { rest }, "{text}");
    }
    for text in [
        "f(Store { entries }, g(1));",
        "let v = vec![Store { entries }];",
        "Outer { s: Store { entries }, n: 1 }",
        "let s = Store { entries }.into_inner();",
        "x => Store { entries },",
        "Store { entries, ..Store::new() }",
    ] {
        assert_eq!(kind(text), Braces::Literal, "{text}");
    }
}

#[test]
fn a_literal_gets_the_field_first_in_its_own_shape() {
    let one_line = "Self { entries: Vec::new() }";
    let (at, len, text) = literal_insertion(one_line, 5, "cap: 64");
    let mut out = one_line.to_string();
    out.replace_range(at..at + len, &text);
    assert_eq!(out, "Self { cap: 64, entries: Vec::new() }");

    let lines = "Store {\n            entries,\n        }";
    let (at, len, text) = literal_insertion(lines, 6, "cap: 64");
    let mut out = lines.to_string();
    out.replace_range(at..at + len, &text);
    assert_eq!(
        out,
        "Store {\n            cap: 64,\n            entries,\n        }"
    );

    let empty = "Unit {}";
    let (at, len, text) = literal_insertion(empty, 5, "cap: 64");
    let mut out = empty.to_string();
    out.replace_range(at..at + len, &text);
    assert_eq!(out, "Unit { cap: 64 }");
}

#[test]
fn only_a_name_followed_by_braces_that_build_a_value_is_a_constructor() {
    let at = |t: &str| t.find("Store").unwrap();
    let t = "let s = Store { entries };";
    assert_eq!(
        constructor_brace(t, at(t), "Store"),
        Some(t.find('{').unwrap())
    );
    let t = "let s = Store::<u8> { entries };";
    assert_eq!(
        constructor_brace(t, at(t), "Store"),
        Some(t.find('{').unwrap())
    );
    for t in [
        "impl Store {",
        "impl Default for Store {",
        "fn new() -> Store {",
        "pub struct Store {",
        "let s: Store = x;",
        "Store::new()",
        "StoreKey { a }",
    ] {
        assert_eq!(constructor_brace(t, at(t), "Store"), None, "{t}");
    }
    let blocks = impl_blocks(SOURCE);
    let (_, _, open, close) = blocks[0];
    let found = self_literals(SOURCE, open, close);
    assert_eq!(found, vec![SOURCE.find("Self { entries").unwrap() + 5]);
}

#[test]
fn a_word_is_mentioned_only_whole() {
    assert!(mentions("&self", "self"));
    assert!(mentions("self.x + 1", "self"));
    assert!(!mentions("myself.x", "self"));
    assert!(!mentions("selfish", "self"));
}

fn report() -> ExtractedField {
    ExtractedField {
        owner: "Store".into(),
        method: "limit".into(),
        root: "/root".into(),
        file: "src/store.rs".into(),
        name: "cap".into(),
        ty: "Vec<u8>".into(),
        init: "64 * 1024".into(),
        replaced: 1,
        constructors: 2,
        blocked: vec!["src/app.rs:4:9 a pattern that lists every field no longer matches: `let Store { entries } = s;`".into()],
        unmatched: vec!["src/app.rs:1:5 (the analyzer places `Store` here, but the file says otherwise)".into()],
        rewritten: vec![("/root/src/store.rs".into(), "fn main() {}\n".into())],
        diagnostics: vec!["cannot find value `n` (src/app.rs:9:12)".into()],
        applied: false,
    }
}

#[test]
fn the_report_says_where_the_value_went_and_what_was_left() {
    let text = report().render(10_000);
    assert!(text.contains("new field: `cap: Vec<u8>`"), "{text}");
    assert!(
        text.contains("`limit` now reads `self.cap` in 1 place(s)"),
        "{text}"
    );
    assert!(
        text.contains("2 construction site(s) initialise it with `64 * 1024`"),
        "{text}"
    );
    assert!(
        text.contains("stop compiling with one more field"),
        "{text}"
    );
    assert!(text.contains("this could not read"), "{text}");
    assert!(text.contains("Pass `init`"), "{text}");
    assert!(
        text.contains("not one of the primitive `Copy` types"),
        "{text}"
    );
    assert!(text.contains("nothing was written"), "{text}");

    let mut done = report();
    done.ty = "usize".into();
    done.blocked.clear();
    done.unmatched.clear();
    done.diagnostics.clear();
    done.applied = true;
    let text = done.render(10);
    assert!(text.contains("0 errors"), "{text}");
    assert!(!text.contains("Copy"), "{text}");
    assert!(text.contains("diff truncated"), "{text}");
    assert!(text.contains("[applied to 1 file(s)]"), "{text}");
}
