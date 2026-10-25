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
fn coverage_declaration_shapes() {
    for (lang, text, expected) in [
        (Language::Go, "func ready() bool {", true),
        (Language::Go, "func (v Value) ready() bool {", true),
        (Language::Go, "func (v Value ready()", false),
        (Language::Go, "ready()", false),
        (Language::Python, "async def ready():", true),
        (Language::Python, "def other():", false),
        (Language::Swift, "public func ready() -> Bool {", true),
        (
            Language::TypeScript,
            "export default async function ready() {",
            true,
        ),
        (Language::JavaScript, "ready() {", true),
        (Language::TypeScript, "const ready = () => true;", false),
        (Language::JavaScript, "let ready = true;", false),
        (Language::JavaScript, "var ready = true;", false),
        (Language::JavaScript, "return ready();", false),
        (Language::Cpp, "bool ready() {", true),
        (Language::C, "return ready();", false),
        (Language::Cpp, "bool x = ready();", false),
        (Language::Rust, "fn ready() -> bool {", false),
    ] {
        assert_eq!(is_function_decl(text, lang, "ready"), expected, "{text}");
    }
}

#[test]
fn coverage_locate_symbols_lines_and_malformed_bodies() {
    for (lang, text, symbol) in [
        (
            Language::Python,
            "def ready():\n    return True\n",
            "Container.ready",
        ),
        (
            Language::Go,
            "func ready () bool { return true }",
            "Container::ready",
        ),
        (
            Language::TypeScript,
            "function ready<T>() { return true; }",
            "ready",
        ),
    ] {
        for line in [None, Some(1)] {
            let got = find_polyglot_predicate_declaration(text, lang, line, Some(symbol)).unwrap();
            assert_eq!(got.fn_name, "ready");
            assert_eq!(got.decl_name_at, text.find("ready").unwrap());
            assert_eq!(&text[got.close_paren..=got.close_paren], ")");
            assert_eq!(
                &text[got.body_open..=got.body_open],
                if lang == Language::Python { ":" } else { "{" }
            );
            assert!(got.body_close > got.body_open);
        }
    }
    let text = "def ready():\n    return True\n";
    assert_eq!(
        find_polyglot_predicate_declaration(text, Language::Python, Some(1), None)
            .unwrap()
            .decl_name_at,
        4
    );
    for line in [None, Some(0), Some(99)] {
        assert_eq!(
            find_polyglot_predicate_declaration(text, Language::Python, line, None)
                .err()
                .unwrap()
                .to_string(),
            "could not determine predicate name to invert"
        );
    }
    for text in [
        "",
        "already() {}",
        "ready(",
        "ready()",
        "ready() {",
        "ready<",
    ] {
        assert_eq!(
            find_polyglot_predicate_declaration(text, Language::Go, None, Some("ready"))
                .err()
                .unwrap()
                .to_string(),
            "could not find declaration for predicate `ready`"
        );
    }
    assert_eq!(
        find_polyglot_predicate_declaration("def ready()", Language::Python, None, Some("ready"))
            .err()
            .unwrap()
            .to_string(),
        "could not find declaration for predicate `ready`"
    );
}
