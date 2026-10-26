/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::super::decl::insert_generic_decl;
use super::super::param::rewrite_param_entry;
use super::super::syntax::find_polyglot_func_decl;
use super::super::types::PolyglotFuncDecl;
use crate::parameter_object::{Kind, Language, Param};

fn param(name: &str, ty: Option<&str>) -> Param {
    Param {
        raw: name.to_string(),
        name: name.to_string(),
        name_at: 0,
        ty: ty.map(str::to_string),
        default: None,
        optional: false,
        shares_type: false,
        kind: Kind::Plain,
        label: None,
    }
}

fn generified(text: &str, lang: Language, symbol: &str, ty: &str, bound: &str) -> String {
    let decl = find_polyglot_func_decl(text, lang, Some(symbol), None).unwrap();
    let mut out = text.to_string();
    insert_generic_decl(&mut out, text, &decl, ty, bound, lang);
    out
}

#[test]
fn the_type_parameter_is_declared_in_each_languages_own_syntax() {
    // (language, symbol, source, bound, expected)
    let cases: &[(Language, &str, &str, &str, &str)] = &[
        (
            Language::TypeScript,
            "id",
            "function id(x: number) {}",
            "Comparable",
            "function id<T extends Comparable>(x: number) {}",
        ),
        (
            Language::TypeScript,
            "id",
            "function id(x: number) {}",
            "any",
            "function id<T>(x: number) {}",
        ),
        (
            Language::JavaScript,
            "id",
            "function id(x) {}",
            "",
            "function id<T>(x) {}",
        ),
        (
            Language::TypeScript,
            "id",
            "function id<A>(x: number) {}",
            "B",
            "function id<A, T extends B>(x: number) {}",
        ),
        (
            Language::Python,
            "f",
            "def f(x): pass",
            "Hashable",
            "def f[T: Hashable](x): pass",
        ),
        (
            Language::Python,
            "f",
            "def f(x): pass",
            "Any",
            "def f[T](x): pass",
        ),
        (
            Language::Python,
            "f",
            "def f(x): pass",
            "object",
            "def f[T](x): pass",
        ),
        (
            Language::Python,
            "f",
            "def f[A](x): pass",
            "",
            "def f[A, T](x): pass",
        ),
        (
            Language::Swift,
            "f",
            "func f(x: Int) {}",
            "Equatable",
            "func f<T: Equatable>(x: Int) {}",
        ),
        (
            Language::Swift,
            "f",
            "func f(x: Int) {}",
            "Any",
            "func f<T>(x: Int) {}",
        ),
        (
            Language::Swift,
            "f",
            "func f<A>(x: Int) {}",
            "",
            "func f<A, T>(x: Int) {}",
        ),
        (
            Language::Go,
            "F",
            "func F(x int) {}",
            "",
            "func F[T any](x int) {}",
        ),
        (
            Language::Go,
            "F",
            "func F(x int) {}",
            "Number",
            "func F[T Number](x int) {}",
        ),
        (
            Language::Go,
            "F",
            "func F[A any](x int) {}",
            "Number",
            "func F[A any, T Number](x int) {}",
        ),
        (
            Language::Cpp,
            "frob",
            "void frob(int x) {}",
            "",
            "template<typename T>\nvoid frob(int x) {}",
        ),
        (
            Language::Cpp,
            "frob",
            "  void frob(int x) {}",
            "class",
            "  template<typename T>\n  void frob(int x) {}",
        ),
        (
            Language::C,
            "frob",
            "void frob(int x) {}",
            "Integral",
            "template<Integral T>\nvoid frob(int x) {}",
        ),
        (
            Language::Cpp,
            "frob",
            "template <typename U>\nvoid frob(U u) {}",
            "typename",
            "template <typename U, typename T>\nvoid frob(U u) {}",
        ),
    ];
    for (lang, symbol, text, bound, want) in cases {
        assert_eq!(
            generified(text, *lang, symbol, "T", bound),
            *want,
            "{lang:?} {text:?}"
        );
    }
}

#[test]
fn java_puts_the_type_parameter_in_front_of_the_declaration() {
    let text = "int frob(int x) {}";
    let mut decl = PolyglotFuncDecl {
        name: "frob".into(),
        decl_start: 0,
        name_start: 4,
        name_end: 8,
        open_paren: 8,
        close_paren: 14,
        has_generics: false,
        generics_span: None,
    };
    for (bound, want) in [
        ("Number", "<T extends Number> int frob(int x) {}"),
        ("Object", "<T> int frob(int x) {}"),
        ("", "<T> int frob(int x) {}"),
    ] {
        let mut out = text.to_string();
        insert_generic_decl(&mut out, text, &decl, "T", bound, Language::Java);
        assert_eq!(out, want, "{bound:?}");
    }
    let text = "<A> int frob(int x) {}";
    decl.has_generics = true;
    decl.generics_span = Some((1, 2));
    let mut out = text.to_string();
    insert_generic_decl(&mut out, text, &decl, "T", "Number", Language::Java);
    assert_eq!(out, "<A, T extends Number> int frob(int x) {}");
}

#[test]
fn a_parameter_entry_takes_the_type_parameter_keeping_its_container_and_default() {
    // (language, entry, declared type, expected)
    let cases: &[(Language, &str, Option<&str>, &str)] = &[
        (Language::TypeScript, "x: string", None, "x: T"),
        (Language::TypeScript, "x?: string", None, "x?: T"),
        (Language::TypeScript, "x: string[]", None, "x: T[]"),
        (
            Language::TypeScript,
            "x: Array<string>",
            None,
            "x: Array<T>",
        ),
        (Language::TypeScript, "x: Foo<Bar", None, "x: T"),
        (Language::TypeScript, "x: string = 'a'", None, "x: T = 'a'"),
        (Language::TypeScript, "x = 5", None, "x: T = 5"),
        (Language::JavaScript, "x", None, "x: T"),
        (Language::Python, "x: int", None, "x: T"),
        (Language::Python, "x: list[int]", None, "x: list[T]"),
        (Language::Python, "x: dict[str", None, "x: T"),
        (Language::Python, "x: int = 3", None, "x: T = 3"),
        (Language::Python, "x = 1", None, "x: T = 1"),
        (Language::Python, "x", None, "x: T"),
        (Language::Swift, "x: Int", None, "x: T"),
        (Language::Swift, "x: Int?", None, "x: T?"),
        (Language::Swift, "x: [Int]", None, "x: [T]"),
        (Language::Swift, "x: inout Int", None, "x: inout T"),
        (Language::Swift, "a b: Int = 5", None, "a b: T = 5"),
        (Language::Swift, "x", None, "x: T"),
        (Language::Go, "x int", Some("int"), "x T"),
        (Language::Go, "x *Foo", Some("*Foo"), "x *T"),
        (Language::Go, "x []Foo", Some("[]Foo"), "x []T"),
        (Language::Go, "xs ...Foo", Some("...Foo"), "xs ...T"),
        (Language::Go, "x", None, "x T"),
        (Language::Cpp, "int x", None, "T x"),
        (Language::Cpp, "const std::string& x", None, "const T& x"),
        (Language::C, "struct Foo x", None, "struct T x"),
        (Language::Cpp, "int x = 5", None, "T x= 5"),
        (Language::Cpp, "int xx", None, "int xx"),
        (Language::Java, "String x", None, "T x"),
    ];
    for (lang, entry, ty, want) in cases {
        let got = rewrite_param_entry(
            entry,
            &param(if entry.starts_with("xs") { "xs" } else { "x" }, *ty),
            "T",
            *lang,
        );
        assert_eq!(got, *want, "{lang:?} {entry:?}");
    }
}
