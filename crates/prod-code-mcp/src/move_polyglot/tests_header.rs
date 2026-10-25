/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

use super::header::{extract_polyglot_decl_name, is_polyglot_decl_header};

use Language::{C, Cpp, Go, Java, JavaScript, Python, Rust, Swift, TypeScript};

#[test]
fn decl_header_table() {
    let cases: &[(Language, &str, bool)] = &[
        (Python, "def f():", true),
        (Python, "async def f():", true),
        (Python, "class A:", true),
        (Python, "    def method(self):", false),
        (Python, "x = 1", false),
        (Python, "   ", false),
        (TypeScript, "import x from \"y\";", false),
        (TypeScript, "export { a, b };", false),
        (TypeScript, "export * from \"x\";", false),
        (TypeScript, "export default foo;", true),
        (TypeScript, "export function f() {}", true),
        (TypeScript, "export async function f() {}", true),
        (TypeScript, "async function f() {}", true),
        (TypeScript, "class A {}", true),
        (TypeScript, "export interface I {}", true),
        (TypeScript, "type T = number;", true),
        (TypeScript, "export enum E {}", true),
        (TypeScript, "export const c = 1;", true),
        (TypeScript, "let l = 1;", true),
        (TypeScript, "export var v = 1;", true),
        (JavaScript, "return value;", false),
        (Go, "func f() {", true),
        (Go, "type T struct {", true),
        (Go, "var x = 1", true),
        (Go, "const c = 1", true),
        (Go, "\tfunc nested() {", false),
        (Go, "import \"fmt\"", false),
        (Swift, "public func f() {", true),
        (Swift, "open class K {", true),
        (Swift, "internal enum E {", true),
        (Swift, "fileprivate protocol P {", true),
        (Swift, "private struct S {", true),
        (Swift, "let x = 1", false),
        (Cpp, "class A {", true),
        (Cpp, "struct S", true),
        (Cpp, "enum E {", true),
        (Cpp, "int f(int a) {", true),
        (Cpp, "int f(int a)", true),
        (Cpp, "int f(", false),
        (Cpp, "#include <x>", false),
        (Cpp, "using namespace std;", false),
        (Cpp, "  int f() {", false),
        (C, "int x = 1;", false),
        (Rust, "fn f() {}", false),
        (Java, "public class A {", true),
        (Java, "protected void f() {", true),
        (Java, "private int x;", true),
        (Java, "interface I {", true),
        (Java, "record R(int a) {", true),
        (Java, "enum E {", true),
        (Java, "int x;", false),
    ];
    for (idx, (lang, line, expected)) in cases.iter().enumerate() {
        assert_eq!(
            is_polyglot_decl_header(line, *lang),
            *expected,
            "case {idx}: {line:?}"
        );
    }
}

#[test]
fn decl_name_table() {
    let cases: &[(Language, &str, Option<&str>)] = &[
        (Python, "def foo(a):", Some("foo")),
        (Python, "async def bar(x):", Some("bar")),
        (Python, "class Baz(Base):", Some("Baz")),
        (Python, "class Qux:", Some("Qux")),
        (Python, "def bare", Some("bare")),
        (Python, "x = 1", None),
        (TypeScript, "export default function foo() {}", Some("foo")),
        (TypeScript, "export async function bar() {}", Some("bar")),
        (TypeScript, "export class A {", Some("A")),
        (TypeScript, "const x = 1;", Some("x")),
        (TypeScript, "type T = number;", Some("T")),
        (TypeScript, "enum E {", Some("E")),
        (JavaScript, "let y = 2", Some("y")),
        (TypeScript, "interface I {}", Some("I")),
        (TypeScript, "export default function () {}", None),
        (TypeScript, "export default {", None),
        (Go, "func foo(a int) int {", Some("foo")),
        (Go, "func (s *S) Method() {", Some("Method")),
        (Go, "func Generic[T any](x T) T {", Some("Generic")),
        (Go, "type T struct {", Some("T")),
        (Go, "var x = 1", Some("x")),
        (Go, "const c int = 1", Some("c")),
        (Go, "const (", None),
        (Go, "func (", None),
        (Go, "import \"fmt\"", None),
        (Swift, "public func foo(x: Int) {", Some("foo")),
        (Swift, "struct Point {", Some("Point")),
        (Swift, "private class Hidden {", Some("Hidden")),
        (Swift, "protocol P {", Some("P")),
        (Swift, "enum E: Int {", Some("E")),
        (Swift, "let x = 1", None),
        (Cpp, "int foo(int a) {", Some("foo")),
        (Cpp, "void Klass::method() {", Some("method")),
        (Cpp, "char *f(void)", Some("f")),
        (Cpp, "if (x) {", None),
        (Cpp, "class Widget {", Some("Widget")),
        (C, "struct S;", Some("S")),
        (C, "int x = 1;", None),
        (C, "(x)", None),
        (Rust, "fn f() {}", None),
        (Java, "public void run() {", Some("run")),
        (Java, "if (x)", None),
        (Java, "public class Foo extends Bar {", Some("Foo")),
        (Java, "interface I {", Some("I")),
        (Java, "record R(int a) {", Some("R")),
        (Java, "public enum E {", Some("E")),
        (Java, "private int x;", None),
    ];
    for (idx, (lang, line, expected)) in cases.iter().enumerate() {
        assert_eq!(
            extract_polyglot_decl_name(line, *lang).as_deref(),
            *expected,
            "case {idx}: {line:?}"
        );
    }
}
