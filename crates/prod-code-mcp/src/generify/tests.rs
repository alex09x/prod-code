/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{find_polyglot_func_decl, generics_span, split_reference};
use super::types::Generified;
use crate::parameter_object::Language;

#[test]
fn symbol_selection_respects_the_resolved_declaration_line() {
    let text = "function convert(value: string): string { return value; }\n\
function convert(value: number): number { return value; }";
    let declaration =
        find_polyglot_func_decl(text, Language::TypeScript, Some("convert"), Some(2)).unwrap();
    assert_eq!(
        &text[declaration.open_paren + 1..declaration.close_paren],
        "value: number"
    );
}

#[test]
fn a_reference_is_kept_in_front_of_the_type_parameter() {
    assert_eq!(
        split_reference("Vec<u32>"),
        (String::new(), "Vec<u32>".to_string())
    );
    assert_eq!(
        split_reference("&Vec<u32>"),
        ("&".to_string(), "Vec<u32>".to_string())
    );
    assert_eq!(
        split_reference("&mut String"),
        ("&mut ".to_string(), "String".to_string())
    );
    assert_eq!(
        split_reference("&'a str"),
        ("&'a ".to_string(), "str".to_string())
    );
    assert_eq!(
        split_reference("&'a mut Buf"),
        ("&'a mut ".to_string(), "Buf".to_string())
    );
}

#[test]
fn the_generic_list_is_found_after_the_name() {
    let t = "fn f<A: Clone, B>(a: A) {}";
    let (s, e) = generics_span(t, 4).unwrap();
    assert_eq!(&t[s..e], "A: Clone, B");
    let t = "fn g<M: Into<Vec<u8>>>(m: M) {}";
    let (s, e) = generics_span(t, 4).unwrap();
    assert_eq!(&t[s..e], "M: Into<Vec<u8>>");
    assert!(generics_span("fn h(x: u8) {}", 4).is_none());
}

#[test]
fn the_report_says_what_the_signature_became_and_who_was_checked() {
    let done = Generified {
        function: "total".into(),
        root: "/root".into(),
        file: "src/lib.rs".into(),
        was: "fn total(v: &Vec<u32>)".into(),
        now: "fn total<T: AsRef<[u32]>>(v: &T)".into(),
        callers_checked: 2,
        rewritten: vec![],
        diagnostics: vec!["no method named `iter` found (src/lib.rs:2:7)".into()],
        applied: false,
    };
    let text = done.render();
    assert!(
        text.contains("now: `fn total<T: AsRef<[u32]>>(v: &T)`"),
        "{text}"
    );
    assert!(text.contains("2 file(s) that call it checked"), "{text}");
    assert!(text.contains("the bound does not"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    let mut ok = done.clone();
    ok.diagnostics.clear();
    ok.applied = true;
    assert!(ok.render().contains("0 errors") && ok.render().contains("[applied]"));
}
