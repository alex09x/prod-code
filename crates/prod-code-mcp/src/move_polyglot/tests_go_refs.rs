/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::go_imports::{find_go_type_rhs, is_in_go_string_or_comment, rewrite_go_call_sites};

#[test]
fn string_and_comment_detection() {
    for (text, inside) in [
        ("x := \"a Foo\"", true),
        ("x := \"a\\\" Foo\"", true),
        ("x := `a \\ Foo`", true),
        ("/* Foo */", true),
        ("/* c */ Foo", false),
        ("// Foo", true),
        ("// c\nFoo", false),
        ("'\"' Foo", false),
        ("x := \"a\" + Foo", false),
    ] {
        let at = text.find("Foo").unwrap();
        assert_eq!(is_in_go_string_or_comment(text, at), inside, "{text:?}");
    }
}

#[test]
fn type_right_hand_side_lookup() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("type G[T any] []T", "G", Some(" []T")),
        ("type A = map[string]int", "A", Some("= map[string]int")),
        (
            "type (\n\tK map[string]int\n)\n",
            "K",
            Some("map[string]int\n"),
        ),
        ("type Kx int", "K", None),
        ("// type K int\n", "K", None),
        ("var typeK int", "K", None),
    ];
    for (idx, (text, name, expected)) in cases.iter().enumerate() {
        assert_eq!(find_go_type_rhs(text, name), *expected, "case {idx}");
    }
}

#[test]
fn call_site_rewrites() {
    let cases: &[(&str, &str, &str, usize)] = &[
        (
            "package p\nfunc main() { Foo(1) }\n",
            "Foo",
            "package p\nfunc main() { lib.Foo(1) }\n",
            1,
        ),
        (
            "// Foo\nx := \"Foo\" + Foo()\n",
            "Foo",
            "// Foo\nx := \"Foo\" + lib.Foo()\n",
            1,
        ),
        ("a.Foo()\nFoo()", "Foo", "a.Foo()\nlib.Foo()", 1),
        (
            "func Foo() {}\nFoo()\n",
            "Foo",
            "func Foo() {}\nlib.Foo()\n",
            1,
        ),
        (
            "func (r R) Foo() {}\nFoo()\n",
            "Foo",
            "func (r R) Foo() {}\nlib.Foo()\n",
            1,
        ),
        ("var x Foo\n", "Foo", "var x lib.Foo\n", 1),
        ("package Foo\nFoo()\n", "Foo", "package Foo\nlib.Foo()\n", 1),
        ("Foo(Foo())", "Foo", "lib.Foo(lib.Foo())", 2),
        (
            "type S struct {\n\tFoo int\n}\n",
            "Foo",
            "type S struct {\n\tFoo int\n}\n",
            0,
        ),
        ("x := S{Foo: 1}\n", "Foo", "x := S{Foo: 1}\n", 0),
        (
            "m := map[string]int{Foo: 1}\n",
            "Foo",
            "m := map[string]int{lib.Foo: 1}\n",
            1,
        ),
        (
            "type M map[string]int\nvar m = M{Foo: 1}\n",
            "Foo",
            "type M map[string]int\nvar m = M{lib.Foo: 1}\n",
            1,
        ),
        (
            "switch x {\ncase Foo:\n}\n",
            "Foo",
            "switch x {\ncase lib.Foo:\n}\n",
            1,
        ),
        ("a := []int{Foo: 1}\n", "Foo", "a := []int{lib.Foo: 1}\n", 1),
        ("abc", "", "abc", 0),
        ("x := Foobar + FooX\n", "Foo", "x := Foobar + FooX\n", 0),
    ];
    for (idx, (text, name, expected, count)) in cases.iter().enumerate() {
        let (out, n) =
            rewrite_go_call_sites(text, name, "lib").unwrap_or_else(|e| panic!("case {idx}: {e}"));
        assert_eq!((out.as_str(), n), (*expected, *count), "case {idx}");
    }
}

#[test]
fn call_site_refusals() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "var Foo = 1\n",
            "Foo",
            "shadowed by a local `var` declaration",
        ),
        (
            "const Foo = 1\n",
            "Foo",
            "shadowed by a local `const` declaration",
        ),
        (
            "type Foo struct{}\n",
            "Foo",
            "shadowed by a local `type` declaration",
        ),
        ("Foo := 1\n", "Foo", "shadowed by a local binding"),
        ("a, Foo := 1, 2\n", "Foo", "shadowed by a local binding"),
        (
            "func f(Foo int) {}\n",
            "Foo",
            "shadowed by a parameter declaration",
        ),
        (
            "func f(a int, Foo string) {}\n",
            "Foo",
            "shadowed by a parameter declaration",
        ),
        (
            "func f(a, Foo ...int) {}\n",
            "Foo",
            "shadowed by a parameter declaration",
        ),
        (
            "func f(Foo []int) {}\n",
            "Foo",
            "shadowed by a parameter declaration",
        ),
        (
            "var x Foo int\n",
            "Foo",
            "shadowed by a parameter declaration",
        ),
        (
            "foo()\n",
            "foo",
            "Go symbol `foo` at byte 0 is unexported and cannot be qualified across packages",
        ),
    ];
    for (idx, (text, name, message)) in cases.iter().enumerate() {
        let err = rewrite_go_call_sites(text, name, "lib").expect_err("must refuse");
        assert!(err.to_string().contains(message), "case {idx}: {err}");
    }
}
