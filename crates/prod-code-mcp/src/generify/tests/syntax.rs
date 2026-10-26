/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::path::Path;

use super::super::syntax::{
    display, find_polyglot_func_decl, generics_span, is_ident, matching_angle_bracket,
    split_reference,
};
use crate::parameter_object::Language;

fn slices(text: &str, lang: Language, symbol: &str) -> (String, String, Option<String>) {
    let d = find_polyglot_func_decl(text, lang, Some(symbol), None).unwrap();
    (
        text[d.decl_start..d.name_end].to_string(),
        text[d.open_paren + 1..d.close_paren].to_string(),
        d.generics_span.map(|(s, e)| text[s..e].to_string()),
    )
}

fn owned(a: &str, b: &str, c: Option<&str>) -> (String, String, Option<String>) {
    (a.to_string(), b.to_string(), c.map(str::to_string))
}

#[test]
fn a_declaration_is_found_with_its_parameters_and_generics_in_every_language() {
    let cases = [
        (
            Language::TypeScript,
            "id",
            "function id<T>(x: T): T {}",
            owned("function id", "x: T", Some("T")),
        ),
        (
            Language::TypeScript,
            "id",
            "function id (x: T) {}",
            owned("function id", "x: T", None),
        ),
        (
            Language::JavaScript,
            "f",
            "const f = <T,>(x: T) => x;",
            owned("const f", "x: T", Some("T,")),
        ),
        (
            Language::TypeScript,
            "g",
            "const g = async (x: number) => x;",
            owned("const g", "x: number", None),
        ),
        (
            Language::TypeScript,
            "g",
            "let g = async <T>(x: T) => x;",
            owned("let g", "x: T", Some("T")),
        ),
        (
            Language::Swift,
            "id",
            "func id<T>(x: T) {}",
            owned("func id", "x: T", Some("T")),
        ),
        (
            Language::Python,
            "first",
            "def first[T](xs: list[T]) -> T:",
            owned("def first", "xs: list[T]", Some("T")),
        ),
        (
            Language::Python,
            "f",
            "def f (x): pass",
            owned("def f", "x", None),
        ),
        (
            Language::Go,
            "Map",
            "func Map[T any](x T) T {",
            owned("func Map", "x T", Some("T any")),
        ),
        (
            Language::Go,
            "Id",
            "func Id(x int) int {",
            owned("func Id", "x int", None),
        ),
        (
            Language::Cpp,
            "frob",
            "void frob(int x) {}",
            owned("void frob", "int x", None),
        ),
        (
            Language::Cpp,
            "frob",
            "template <typename U>\nvoid frob(U u) {}",
            owned(
                "template <typename U>\nvoid frob",
                "U u",
                Some("typename U"),
            ),
        ),
        (
            Language::C,
            "frob",
            "template <class A> void other();\nvoid frob(int x) {}",
            owned("void frob", "int x", None),
        ),
    ];
    for (lang, symbol, text, want) in cases {
        assert_eq!(slices(text, lang, symbol), want, "{lang:?} {text:?}");
    }
}

#[test]
fn a_qualified_symbol_and_a_line_alone_name_the_function() {
    let text = "function a() {}\nfunction target(x) {}\n";
    for symbol in ["Owner::target", "obj.target", " target "] {
        let d = find_polyglot_func_decl(text, Language::TypeScript, Some(symbol), None).unwrap();
        assert_eq!(d.name, "target", "{symbol}");
        assert_eq!(&text[d.open_paren + 1..d.close_paren], "x");
    }
    let d = find_polyglot_func_decl(text, Language::TypeScript, None, Some(2)).unwrap();
    assert_eq!(d.name, "target");
    assert_eq!(&text[d.decl_start..d.name_end], "function target");
    // Line 1 names `target` too (the nearest declaration within two lines wins), but that
    // function is not declared on line 1.
    let err = find_polyglot_func_decl(text, Language::TypeScript, None, Some(1)).unwrap_err();
    assert_eq!(
        err.to_string(),
        "could not find declaration of `target` in file"
    );
}

#[test]
fn what_is_not_a_declaration_is_skipped_or_refused() {
    let real = "function frob(a: number) {}\n";
    for decoy in [
        "// frob(x)\n",
        "import { frob } from 'x';\n",
        "from m import frob\n",
        "export { frob };\n",
        "export * from 'frob';\n",
        "use a::frob;\n",
        "#include \"frob\"\n",
        "const y = frob(1);\n",
        "y = 1 + frob(1);\n",
        "call(a, frob(1));\n",
        "label: frob(1);\n",
        "return frob(1);\n",
        "xfrob(1);\n",
        "frobnicate(1);\n",
    ] {
        let text = format!("{decoy}{real}");
        let d = find_polyglot_func_decl(&text, Language::TypeScript, Some("frob"), None).unwrap();
        assert_eq!(
            &text[d.open_paren + 1..d.close_paren],
            "a: number",
            "{decoy:?}"
        );
        assert!(d.decl_start >= decoy.len(), "{decoy:?}");
    }
    let refusals = [
        (
            "function other() {}",
            Some("nope"),
            None,
            "could not find declaration of `nope` in file",
        ),
        (
            "function frob(a",
            Some("frob"),
            None,
            "could not find declaration of `frob` in file",
        ),
        (
            "const h = 5;",
            Some("h"),
            None,
            "could not find declaration of `h` in file",
        ),
        (
            "function a() {}",
            None,
            None,
            "could not determine function name to generify parameter for",
        ),
        (
            "let x = 1;\n",
            None,
            Some(1),
            "could not determine function name to generify parameter for",
        ),
    ];
    for (text, symbol, line, want) in refusals {
        let err = find_polyglot_func_decl(text, Language::TypeScript, symbol, line).unwrap_err();
        assert_eq!(err.to_string(), want, "{text:?}");
    }
}

#[test]
fn angle_brackets_nest_and_must_close() {
    assert_eq!(matching_angle_bracket("<A<B>>", 0), Some(5));
    assert_eq!(matching_angle_bracket("x<A>", 1), Some(3));
    assert_eq!(matching_angle_bracket("x<A>", 0), None);
    assert_eq!(matching_angle_bracket("<A<B>", 0), None);
    assert_eq!(matching_angle_bracket("<A>", 9), None);
    // A return arrow in a bound closes nothing (#1382).
    let t = "fn run<F: Fn() -> i32>(f: F, value: String) -> i32 {";
    let (s, e) = generics_span(t, 6).unwrap();
    assert_eq!(&t[s..e], "F: Fn() -> i32");
    let t = "fn g<F: Fn(Vec<u8>) -> Option<u8>, G>(f: F) {}";
    let (s, e) = generics_span(t, 4).unwrap();
    assert_eq!(&t[s..e], "F: Fn(Vec<u8>) -> Option<u8>, G");
    let t = "fn f <T>(x: T) {}";
    let (s, e) = generics_span(t, 4).unwrap();
    assert_eq!(&t[s..e], "T");
    assert_eq!(generics_span("fn f<T", 4), None);
    assert_eq!(generics_span("fn f", 4), None);
}

#[test]
fn small_helpers_spell_paths_identifiers_and_references() {
    assert_eq!(
        display(Path::new("/w"), Path::new("/w/src/a.rs")),
        "src/a.rs"
    );
    assert_eq!(display(Path::new("/w"), Path::new("/o/a.rs")), "/o/a.rs");
    assert!(is_ident('é') && is_ident('_') && is_ident('7'));
    assert!(!is_ident('-') && !is_ident(' '));
    assert_eq!(
        split_reference("  &  mut T "),
        ("&mut ".to_string(), "T".to_string())
    );
    assert_eq!(split_reference("&'a"), ("&'a ".to_string(), String::new()));
}
