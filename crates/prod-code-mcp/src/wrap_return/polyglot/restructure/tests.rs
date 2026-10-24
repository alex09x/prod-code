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
use crate::wrap_return::polyglot::find::find_polyglot_decl;

#[test]
fn coverage_restructure_signature_matrix() {
    use Language::*;
    use Wrapper::*;
    for (lang, input, wrapper, expected, now) in [
        (TypeScript, "function f(): number {\nreturn 1;\n}", Promise, "async function f(): Promise<number> {\nreturn 1;\n}", "Promise<number>"),
        (TypeScript, "async function f(): void {}", Promise, "async function f(): Promise<void> {}", "Promise<void>"),
        (TypeScript, "function f() {}", Promise, "async function f() {}", "Promise<inferred>"),
        (JavaScript, "function f() {}", Promise, "async function f() {}", "Promise"),
        (TypeScript, "const f = (): number => {\nreturn 1;\n}", Promise, "const f = async (): Promise<number> => {\nreturn 1;\n}", "Promise<number>"),
        (TypeScript, "f(): number {\nreturn 1;\n}", Promise, "async f(): Promise<number> {\nreturn 1;\n}", "Promise<number>"),
        (TypeScript, "function f() {}", Option, "function f(): void | null {}", "void | null"),
        (JavaScript, "function f() {}", Option, "function f() {}", "Option"),
        (TypeScript, "function f(): number {}", Option, "function f(): number | null {}", "number | null"),
        (TypeScript, "function f() {}", Result, "function f(): Result<void, Error> {}", "Result<void, Error>"),
        (JavaScript, "function f() {}", Result, "function f() {}", "Result<Error>"),
        (TypeScript, "function f(): number {}", Custom("Box<T>".into()), "function f(): Box<number> {}", "Box<number>"),
        (TypeScript, "function f() {}", Custom("Box".into()), "function f(): Box<void> {}", "Box<void>"),
        (JavaScript, "function f() {}", Custom("Box".into()), "function f() {}", "Box"),
        (Python, "def f():\n    pass\n", Option, "def f() -> Optional[Any]:\n    pass\n", "Optional[Any]"),
        (Python, "def f() -> int:\n    pass\n", Result, "def f() -> Result[int, Exception]:\n    pass\n", "Result[int, Exception]"),
        (Python, "def f():\n    pass\n", Result, "def f() -> Result[Any, Exception]:\n    pass\n", "Result[Any, Exception]"),
        (Python, "def f():\n    pass\n", Custom("Box".into()), "def f() -> Box[Any]:\n    pass\n", "Box[Any]"),
        (Python, "def f() -> int:\n    pass\n", Custom("Box[T]".into()), "def f() -> Box[int]:\n    pass\n", "Box[int]"),
        (Python, "def f() -> int:\n    pass\n", Custom("Box".into()), "def f() -> Box[int]:\n    pass\n", "Box[int]"),
        (Cpp, "int f() {}", Option, "std::optional<int> f() {}", "std::optional<int>"),
        (C, "int f() {}", Result, "std::expected<int, std::string> f() {}", "std::expected<int, std::string>"),
        (Cpp, "int f() {}", Custom("Box<T>".into()), "Box<int> f() {}", "Box<int>"),
        (Cpp, "int f() {}", Custom("Box".into()), "Box<int> f() {}", "Box<int>"),
        (Cpp, "void f() {}", Custom("Box".into()), "Box f() {}", "Box"),
        (Swift, "func f() -> Int {}", Option, "func f() -> Int? {}", "Int?"),
        (Swift, "func f() {}", Option, "func f()  -> Void? {}", "Void?"),
        (Swift, "func f() -> Int {}", Result, "func f() -> Result<Int, Error> {}", "Result<Int, Error>"),
        (Swift, "func f() {}", Result, "func f()  -> Result<Void, Error> {}", "Result<Void, Error>"),
        (Swift, "func f() -> Int {}", Custom("Box<>".into()), "func f() -> Box<Int> {}", "Box<Int>"),
        (Swift, "func f() -> Int {}", Custom("Box".into()), "func f() -> Box<Int> {}", "Box<Int>"),
        (Swift, "func f() {}", Custom("Box".into()), "func f()  -> Box<Void> {}", "Box<Void>"),
        (Go, "func f() int {}", Result, "func f() (int, error) {}", "(int, error)"),
        (Go, "func f() (int, bool) {}", Result, "func f() (int, bool, error) {}", "(int, bool, error)"),
        (Go, "func f() {}", Result, "func f()  error {}", "error"),
        (Go, "func f() int {}", Pointer, "func f() *int {}", "*int"),
        (Go, "func f() {}", Pointer, "func f()  * {}", "*"),
        (Go, "func f() int {}", Custom("Box[T]".into()), "func f() Box[int] {}", "Box[int]"),
        (Go, "func f() int {}", Custom("Box".into()), "func f() Box[int] {}", "Box[int]"),
        (Go, "func f() int {}", Custom("*Box".into()), "func f() *Box {}", "*Box"),
        (Go, "func f() {}", Custom("Box".into()), "func f()  Box {}", "Box"),
    ] {
        let decl = find_polyglot_decl(input, lang, Some("f"), None).unwrap();
        let (text, ty) = restructure_declaring_file(input, lang, &decl, &wrapper, None, None).unwrap();
        assert_eq!((text.as_str(), ty.as_str()), (expected, now), "{input} {wrapper:?}");
    }
}

#[test]
fn coverage_restructure_refusals() {
    for (lang, input, wrapper, error) in [
        (Language::TypeScript, "function f() {}", Wrapper::Pointer, "Pointer wrapper is not supported for TypeScript/JavaScript"),
        (Language::Python, "def f():\n    pass", Wrapper::Promise, "Promise wrapper is not supported for Python"),
        (Language::Python, "def f():\n    pass", Wrapper::Pointer, "Pointer wrapper is not supported for Python"),
        (Language::Cpp, "int f() {}", Wrapper::Promise, "Promise wrapper is not supported for C++"),
        (Language::Cpp, "int f() {}", Wrapper::Pointer, "Pointer wrapper is not supported for C++"),
        (Language::Swift, "func f() {}", Wrapper::Promise, "Promise wrapper is not supported for Swift"),
        (Language::Swift, "func f() {}", Wrapper::Pointer, "Pointer wrapper is not supported for Swift"),
        (Language::Go, "func f() {}", Wrapper::Promise, "Promise wrapper is not supported for Go"),
    ] {
        let decl = find_polyglot_decl(input, lang, Some("f"), None).unwrap();
        assert_eq!(restructure_declaring_file(input, lang, &decl, &wrapper, None, None).unwrap_err().to_string(), error);
    }
}
