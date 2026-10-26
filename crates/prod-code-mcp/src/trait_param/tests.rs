/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::*;
use super::types::*;

#[test]
fn a_method_s_owner_is_its_trait_or_the_trait_it_implements() {
    let t = "pub trait Shape {\n    fn area(&self) -> u32;\n}\nimpl<T: Copy> geo::Shape<T> for Circle {\n    fn area(&self) -> u32 { 1 }\n}\nimpl Circle {\n    fn r(&self) {}\n}\nfn free() {}\n";
    assert_eq!(
        owner_of(t, t.find("area").unwrap()),
        Some(Owner::Trait {
            name: "Shape".into()
        })
    );
    let in_impl = t.match_indices("area").nth(1).unwrap().0;
    let Some(Owner::Impl { trait_at }) = owner_of(t, in_impl) else {
        panic!("an implementation of a trait")
    };
    assert!(t[trait_at..].starts_with("Shape<T> for"));
    assert_eq!(owner_of(t, t.find("fn r").unwrap()), None);
    assert_eq!(owner_of(t, t.find("fn free").unwrap()), None);
}

#[test]
fn an_item_is_cut_with_its_comma_and_nothing_else() {
    let list = "&self, scale: u32, f: Box<dyn Fn(u32, u32) -> u32>, last: u8";
    let spans = item_spans(list);
    assert_eq!(spans.len(), 4);
    let cut = |i: usize| {
        let (from, to) = removal(&spans, i).unwrap();
        format!("{}{}", &list[..from], &list[to..])
    };
    assert_eq!(cut(1), "&self, f: Box<dyn Fn(u32, u32) -> u32>, last: u8");
    assert_eq!(cut(3), "&self, scale: u32, f: Box<dyn Fn(u32, u32) -> u32>");
    let multi = "\n    a: u32,\n    b: u32,\n";
    let spans = item_spans(multi);
    let (from, to) = removal(&spans, 1).unwrap();
    assert_eq!(
        format!("{}{}", &multi[..from], &multi[to..]),
        "\n    a: u32,\n"
    );
    let one = item_spans("x");
    assert_eq!(removal(&one, 0), Some((0, 1)));
    assert_eq!(removal(&one, 1), None);
    assert_eq!(item_spans("a < b, c << 2").len(), 2);
}

#[test]
fn an_argument_that_does_something_is_named() {
    assert_eq!(effect_of("7"), None);
    assert_eq!(effect_of("a != b"), None);
    assert_eq!(effect_of("next()"), Some("calls something"));
    assert_eq!(effect_of("vec![1]"), Some("expands a macro"));
    assert_eq!(effect_of("!flag"), None);
    assert_eq!(effect_of("r?"), Some("can return early with `?`"));
    assert_eq!(effect_of("f.await"), Some("awaits"));
    // Assignments are effects wherever they stand; plain blocks are not (#1396).
    assert_eq!(effect_of("{ counter += 1; value }"), Some("assigns"));
    assert_eq!(effect_of("{ x = 2; x }"), Some("assigns"));
    assert_eq!(effect_of("{ let y = a + 1; y * 2 }"), None);
    assert_eq!(
        effect_of("if ready { return } else { 1 }"),
        Some("leaves the surrounding code")
    );
    assert_eq!(effect_of("a == b && c <= d"), None);
    assert_eq!(effect_of("&mut buffer[1..]"), None);
    // Text that is no expression keeps the textual checks.
    assert_eq!(effect_of("next( ,"), Some("calls something"));
    assert!(mentions("let _ = unused;", "unused") && !mentions("_unused", "unused"));
    let t = "trait T { fn a(&self); fn area(&self); }";
    assert_eq!(
        method_in_block(t, 0, "area").map(|i| &t[i..i + 4]),
        Some("area")
    );
    assert_eq!(method_in_block(t, 0, "b"), None);
}
