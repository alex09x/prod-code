/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::lang::Language;
use super::infer_param_type;

/// One row: language, variable name, the enclosing scope text, the inferred type.
type Case = (Language, &'static str, &'static str, Option<&'static str>);

fn check(cases: &[Case]) {
    for (lang, name, scope, expected) in cases {
        assert_eq!(
            infer_param_type(name, scope, *lang).as_deref(),
            *expected,
            "{lang:?} `{name}` in `{scope}`"
        );
    }
}

#[test]
fn dynamic_and_fixed_languages() {
    check(&[
        (Language::Python, "x", "x: int = 3", None),
        (Language::JavaScript, "x", "let x = 3", None),
        (Language::Rust, "x", "let x: String = s;", Some("usize")),
    ]);
}

#[test]
fn typescript_annotations_and_initializers() {
    check(&[
        (
            Language::TypeScript,
            "a",
            "function f(a: number[], b)",
            Some("number[]"),
        ),
        (
            Language::TypeScript,
            "m",
            "(m: Map<K>[], z)",
            Some("Map<K>[]"),
        ),
        (
            Language::TypeScript,
            "b",
            "function f(a: number[], b)",
            Some("any"),
        ),
        (Language::TypeScript, "s", "let s = 'x';", Some("string")),
        (Language::TypeScript, "s", "let s = \"x\";", Some("string")),
        (Language::TypeScript, "s", "let s = `x`;", Some("string")),
        (Language::TypeScript, "n", "let n = 5;", Some("number")),
        (
            Language::TypeScript,
            "t",
            "const t = true;",
            Some("boolean"),
        ),
        (
            Language::TypeScript,
            "t",
            "const t = false;",
            Some("boolean"),
        ),
        (Language::TypeScript, "o", "const o = {};", Some("any")),
        (
            Language::TypeScript,
            "o",
            "const o: {a: 1} = {};",
            Some("any"),
        ),
        (Language::TypeScript, "q", "nothing here", Some("any")),
    ]);
}

#[test]
fn go_annotations_and_short_declarations() {
    check(&[
        (Language::Go, "s", "func f(s string, n int)", Some("string")),
        (Language::Go, "n", "func f(s string, n int)", Some("int")),
        (Language::Go, "p", "func f(p *Item)", Some("*Item")),
        (Language::Go, "xs", "func f(xs []byte)", Some("[]byte")),
        (Language::Go, "e", "func f(e error)", Some("error")),
        (Language::Go, "ok", "func f(ok bool)", Some("bool")),
        (Language::Go, "r", "func f(r rune)", Some("rune")),
        (Language::Go, "x", "x := 3.5", Some("float64")),
        (Language::Go, "n", "n := 42", Some("int")),
        (Language::Go, "s", "s := \"hi\"", Some("string")),
        (Language::Go, "ok", "ok := true", Some("bool")),
        (Language::Go, "ok", "ok := false", Some("bool")),
        (Language::Go, "v", "v := compute()", Some("int")),
        (Language::Go, "q", "unrelated", Some("int")),
    ]);
}

#[test]
fn cpp_and_c_declarations() {
    for lang in [Language::Cpp, Language::C] {
        check(&[
            (lang, "count", "int count = 0;", Some("int")),
            (lang, "ratio", "double ratio;", Some("double")),
            (lang, "f", "float f;", Some("float")),
            (lang, "b", "bool b;", Some("bool")),
            (lang, "n", "size_t n;", Some("size_t")),
            (lang, "l", "long l;", Some("long")),
            (lang, "sh", "short sh;", Some("short")),
            (lang, "u", "unsigned u;", Some("unsigned")),
            (lang, "sg", "signed sg;", Some("signed")),
            (lang, "p", "const char* p;", Some("char*")),
            (lang, "w", "Widget*& w;", Some("Widget*&")),
            (lang, "v", "static v = 1;", Some("int")),
            (lang, "e", "= e", Some("int")),
            (lang, "z", "nothing", Some("int")),
        ]);
    }
}

#[test]
fn swift_and_kotlin_annotations() {
    check(&[
        (
            Language::Swift,
            "a",
            "func f(a: Int, b: Array<Int>)",
            Some("Int"),
        ),
        (
            Language::Swift,
            "b",
            "func f(a: Int, b: Array<Int>)",
            Some("Array<Int>"),
        ),
        (Language::Swift, "c", "func f(c: [Int])", Some("Double")),
        (Language::Swift, "d", "no declaration", Some("Double")),
        (
            Language::Kotlin,
            "a",
            "fun f(a: Int, b : String?)",
            Some("Int"),
        ),
        (
            Language::Kotlin,
            "l",
            "fun f(l: List<Int>[]?, z)",
            Some("List<Int>[]?"),
        ),
        (
            Language::Kotlin,
            "b",
            "fun f(a: Int, b : String?)",
            Some("String?"),
        ),
        (
            Language::Kotlin,
            "m",
            "fun f(m : Map<K>[]?, z)",
            Some("Map<K>[]?"),
        ),
        (Language::Kotlin, "c", "val c = 5", Some("Int")),
        (Language::Kotlin, "d", "val d = 2.5", Some("Double")),
        (Language::Kotlin, "s", "val s = \"x\"", Some("String")),
        (Language::Kotlin, "f", "val f = false", Some("Boolean")),
        (Language::Kotlin, "t", "val t = true", Some("Boolean")),
        (Language::Kotlin, "o", "val o = make()", Some("Any")),
    ]);
}

#[test]
fn zig_annotations() {
    check(&[
        (Language::Zig, "a", "fn f(a: u32, b : ?usize)", Some("u32")),
        (
            Language::Zig,
            "b",
            "fn f(a: u32, b : ?usize)",
            Some("?usize"),
        ),
        (Language::Zig, "c", "fn f(c: *Foo)", Some("*Foo")),
        (Language::Zig, "e", "fn f(e: ![]*?T)", Some("![]*?T")),
        (Language::Zig, "g", "fn f(g : ![]*?T)", Some("![]*?T")),
        (Language::Zig, "d", "fn f(d)", Some("anytype")),
    ]);
}

#[test]
fn java_declarations() {
    check(&[
        (Language::Java, "total", "int total = 0;", Some("int")),
        (Language::Java, "x", "double x;", Some("double")),
        (Language::Java, "x", "float x;", Some("float")),
        (Language::Java, "x", "boolean x;", Some("boolean")),
        (Language::Java, "x", "char x;", Some("char")),
        (Language::Java, "x", "long x;", Some("long")),
        (Language::Java, "x", "short x;", Some("short")),
        (Language::Java, "x", "byte x;", Some("byte")),
        (Language::Java, "x", "String x;", Some("String")),
        (Language::Java, "x", "Object x;", Some("Object")),
        (
            Language::Java,
            "items",
            "List<String>[] items;",
            Some("List<String>[]"),
        ),
        (Language::Java, "v", "var v = 1;", Some("Object")),
        (Language::Java, "x", "return x;", Some("Object")),
        (Language::Java, "q", "nothing", Some("Object")),
    ]);
}

#[test]
fn csharp_declarations() {
    check(&[
        (Language::Csharp, "name", "string name;", Some("string")),
        (Language::Csharp, "age", "int? age;", Some("int?")),
        (Language::Csharp, "x", "double x;", Some("double")),
        (Language::Csharp, "x", "float x;", Some("float")),
        (Language::Csharp, "x", "bool x;", Some("bool")),
        (Language::Csharp, "x", "char x;", Some("char")),
        (Language::Csharp, "x", "long x;", Some("long")),
        (Language::Csharp, "x", "short x;", Some("short")),
        (Language::Csharp, "x", "byte x;", Some("byte")),
        (Language::Csharp, "x", "object x;", Some("object")),
        (
            Language::Csharp,
            "xs",
            "List<int>[] xs;",
            Some("List<int>[]"),
        ),
        (Language::Csharp, "v", "var v = 1;", Some("object")),
        (Language::Csharp, "x", "return x;", Some("object")),
        (Language::Csharp, "q", "nothing", Some("object")),
    ]);
}
