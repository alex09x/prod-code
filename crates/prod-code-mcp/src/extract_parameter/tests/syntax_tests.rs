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

use super::super::enclosing::{name_offset, parameter_list, with_argument};
use super::super::syntax::{Syntax, swift_labeled};

#[test]
fn each_languages_hover_for_a_binding_gives_a_type_that_can_be_written() {
    // What the TypeScript server, basedpyright and gopls answer, verbatim.
    let ts = Syntax::TypeScript;
    assert_eq!(
        ts.type_from_hover("```typescript\nconst cap: number\n```\n")
            .as_deref(),
        Some("number")
    );
    assert_eq!(
        ts.type_from_hover("```typescript\nconst width: 80\n```\n")
            .as_deref(),
        Some("number"),
        "a literal type is widened"
    );
    assert_eq!(
        ts.type_from_hover("```typescript\n(property) Store.entries: number[]\n```\n")
            .as_deref(),
        Some("number[]")
    );
    assert_eq!(
        ts.type_from_hover("```typescript\n(method) Store.limit(): number\n```\n"),
        None,
        "a method's hover names what it returns, not what it is"
    );
    assert_eq!(
        ts.type_from_hover("```typescript\nconst o: {\n    a: number;\n}\n```\n"),
        None
    );

    let py = Syntax::Python;
    assert_eq!(
        py.type_from_hover("```python\n(variable) width: Literal[80]\n```")
            .as_deref(),
        Some("int")
    );
    assert_eq!(
        py.type_from_hover("```python\n(parameter) text: str\n```")
            .as_deref(),
        Some("str")
    );
    assert_eq!(
        py.type_from_hover("```python\n(parameter) self: Self@Store\n```"),
        None
    );
    assert_eq!(
        py.type_from_hover("```python\n(function) def len(\n    obj: Sized,\n    /\n) -> int\n```"),
        None
    );

    let go = Syntax::Go;
    assert_eq!(
        go.type_from_hover("```go\nvar width int\n```").as_deref(),
        Some("int")
    );
    assert_eq!(
        go.type_from_hover("```go\nfield entries []int\n```")
            .as_deref(),
        Some("[]int")
    );
    assert_eq!(
        go.type_from_hover("```go\nconst Base untyped int = 80\n```\n\n---\n\n[`shop.Base` on pkg.go.dev](https://pkg.go.dev/example.com/xp/shop#Base)")
            .as_deref(),
        Some("int")
    );
    assert_eq!(go.type_from_hover("```go\nfunc len(v Type) int\n```"), None);
    assert_eq!(Syntax::JavaScript.type_from_hover("const a: number"), None);
}

#[test]
fn a_literal_has_its_type_in_every_language_but_rust() {
    assert_eq!(Syntax::TypeScript.literal_type("80"), Some("number"));
    assert_eq!(Syntax::TypeScript.literal_type("'x'"), Some("string"));
    assert_eq!(Syntax::Python.literal_type("1.5"), Some("float"));
    assert_eq!(Syntax::Python.literal_type("True"), Some("bool"));
    assert_eq!(Syntax::Go.literal_type("64"), Some("int"));
    assert_eq!(Syntax::Go.literal_type("'x'"), Some("rune"));
    assert_eq!(Syntax::Go.literal_type("\"x\""), Some("string"));
    assert_eq!(Syntax::Go.literal_type("64 * 1024"), None);
    assert_eq!(
        Syntax::Rust.literal_type("80"),
        None,
        "Rust has several integer types"
    );
}

#[test]
fn each_language_spells_the_parameter_its_own_way() {
    assert_eq!(
        Syntax::Rust.parameter("n", Some("usize")).as_deref(),
        Some("n: usize")
    );
    assert_eq!(
        Syntax::TypeScript.parameter("n", Some("number")).as_deref(),
        Some("n: number")
    );
    assert_eq!(Syntax::TypeScript.parameter("n", None), None);
    assert_eq!(
        Syntax::JavaScript.parameter("n", Some("number")).as_deref(),
        Some("n")
    );
    assert_eq!(Syntax::Python.parameter("n", None).as_deref(), Some("n"));
    assert_eq!(
        Syntax::Python.parameter("n", Some("int")).as_deref(),
        Some("n: int")
    );
    assert_eq!(
        Syntax::Go.parameter("n", Some("int")).as_deref(),
        Some("n int")
    );
    assert_eq!(Syntax::Go.parameter("n", None), None);
    assert_eq!(Syntax::of(Path::new("a/b.tsx")), Some(Syntax::TypeScript));
    assert_eq!(Syntax::of(Path::new("a/b.mjs")), Some(Syntax::JavaScript));
    assert_eq!(Syntax::of(Path::new("a/b.swift")), Some(Syntax::Swift));
    assert_eq!(Syntax::of(Path::new("a/b.hpp")), Some(Syntax::Cpp));
    assert_eq!(Syntax::of(Path::new("a/b.h")), Some(Syntax::C));
    assert_eq!(Syntax::of(Path::new("a/b.proto")), None);
    assert_eq!(
        Syntax::C.parameter("pad", Some("int")).as_deref(),
        Some("int pad")
    );
    assert_eq!(
        Syntax::Cpp
            .parameter("label", Some("const char *"))
            .as_deref(),
        Some("const char *label"),
        "the pointer binds to the name"
    );
    assert_eq!(
        Syntax::Cpp
            .parameter("text", Some("const std::string &"))
            .as_deref(),
        Some("const std::string &text")
    );
    assert_eq!(Syntax::C.parameter("pad", None), None);
    assert_eq!(
        Syntax::Swift.parameter("pad", Some("Int")).as_deref(),
        Some("pad: Int")
    );
    assert_eq!(Syntax::Swift.parameter("pad", None), None);
}

#[test]
fn clangd_and_sourcekit_hovers_for_a_binding_give_a_type_that_can_be_written() {
    // What clangd and sourcekit-lsp answer, verbatim.
    let c = Syntax::Cpp;
    assert_eq!(
        c.type_from_hover(
            "### variable `width`\n\n---\nType: `int`\n\nValue = `80 (0x50)`\n\n---\n```cpp\n// In render\nint width = 80\n```"
        )
        .as_deref(),
        Some("int")
    );
    assert_eq!(
        c.type_from_hover(
            "### variable `width`\n\n---\nType: `std::size_t (aka unsigned long)`\n\nValue = `80 (0x50)`\n\nPassed as \\_\\_n\n\n---\n```cpp\n// In render\nstd::size_t width = 80\n```"
        )
        .as_deref(),
        Some("std::size_t"),
        "the name the code wrote, not what it stands for"
    );
    assert_eq!(
        c.type_from_hover(
            "### variable `PREFIX`\n\n---\nType: `const std::string (aka const basic_string<char>)`\n\n---\n```cpp\nstatic const std::string PREFIX = \"> \"\n```"
        )
        .as_deref(),
        Some("std::string"),
        "a const on a value binds nothing the caller sees"
    );
    assert_eq!(
        c.type_from_hover(
            "### param `text`\n\n---\nType: `const std::string & (aka const basic_string<char> &)`\n\n---\n```cpp\n// In render\nconst std::string &text\n```"
        )
        .as_deref(),
        Some("const std::string &"),
        "a const behind a reference stays"
    );
    assert_eq!(
        c.type_from_hover(
            "### function `strlen`\n\nprovided by `<string.h>`\n\n---\n\u{2192} `__size_t (aka unsigned long)`\n\nParameters:\n\n- `const char * __s`\n\nReturn the length of S.\n\n---\n```cpp\nextern __size_t strlen(const char *__s)\n```"
        ),
        None,
        "a function's hover names what it returns, not what it is"
    );

    let swift = Syntax::Swift;
    assert_eq!(
        swift
            .type_from_hover("width\n```swift\nlet width: Int\n```\n\n---\n")
            .as_deref(),
        Some("Int")
    );
    assert_eq!(
        swift
            .type_from_hover("entries\n```swift\nvar entries: [Int]\n```\n\n---\n")
            .as_deref(),
        Some("[Int]")
    );
    assert_eq!(
        swift
            .type_from_hover("count\n```swift\npublic static var count: Int { get }\n```\n")
            .as_deref(),
        Some("Int")
    );
    assert_eq!(
        swift.type_from_hover(
            "render(text:)\n```swift\npublic func render(text: String) -> String\n```\n\n---\n"
        ),
        None
    );
}

#[test]
fn c_and_swift_literals_have_the_type_the_language_gives_them() {
    assert_eq!(Syntax::C.literal_type("80"), Some("int"));
    assert_eq!(Syntax::Cpp.literal_type("0.5"), Some("double"));
    assert_eq!(Syntax::C.literal_type("0.5f"), Some("float"));
    assert_eq!(Syntax::Cpp.literal_type("\"x\""), Some("const char *"));
    assert_eq!(Syntax::C.literal_type("'x'"), Some("char"));
    assert_eq!(Syntax::Cpp.literal_type("true"), Some("bool"));
    assert_eq!(Syntax::C.literal_type("80u"), None);
    assert_eq!(Syntax::Swift.literal_type("80"), Some("Int"));
    assert_eq!(Syntax::Swift.literal_type("0.5"), Some("Double"));
    assert_eq!(Syntax::Swift.literal_type("\"x\""), Some("String"));
    assert_eq!(Syntax::Swift.literal_type("false"), Some("Bool"));
}

#[test]
fn each_server_names_a_function_its_own_way_and_a_caller_writes_the_last_name() {
    assert_eq!(Syntax::Cpp.bare_name("Store::limit"), "limit");
    assert_eq!(Syntax::Swift.bare_name("render(text:)"), "render");
    assert_eq!(Syntax::Swift.bare_name("limit()"), "limit");
    assert_eq!(Syntax::Go.bare_name("(*Store).Limit"), "Limit");
    assert_eq!(Syntax::C.with_parameter("void", "int pad"), "int pad");
    assert_eq!(
        Syntax::C.with_parameter("const char *text", "int pad"),
        "const char *text, int pad"
    );
    assert!(swift_labeled("text: String"));
    assert!(swift_labeled("with text: String"));
    assert!(!swift_labeled("_ text: String"));
}

#[test]
fn the_parameter_list_is_found_after_type_parameters_and_a_go_receiver() {
    let go = "func (s *Store) Limit[T any](n T) int {\n";
    let at = name_offset(go, "Limit", 1, None).expect("the name");
    assert_eq!(&go[at..at + 5], "Limit");
    let (open, close) = parameter_list(go, at + 5).expect("the list");
    assert_eq!(&go[open..close], "n T");

    let ts = "export function pick<F extends () => void>(f: F): F {\n";
    let at = name_offset(ts, "pick", 1, Some((1, 17))).expect("the name");
    let (open, close) = parameter_list(ts, at + 4).expect("the list");
    assert_eq!(&ts[open..close], "f: F");

    // A selection range that does not hold the name is not trusted.
    let py = "def limit(self) -> int:\n";
    assert_eq!(name_offset(py, "limit", 1, Some((1, 1))), Some(4));
}

#[test]
fn a_parameter_after_one_that_collects_the_rest_is_refused() {
    assert_eq!(
        Syntax::TypeScript
            .catch_all("a: number, ...rest: string[]")
            .as_deref(),
        Some("...rest: string[]")
    );
    assert_eq!(
        Syntax::Go.catch_all("a int, xs ...int").as_deref(),
        Some("xs ...int")
    );
    assert_eq!(
        Syntax::Python.catch_all("self, *, key: int").as_deref(),
        Some("*")
    );
    assert_eq!(Syntax::Python.catch_all("self, a: int = 3"), None);
    assert_eq!(Syntax::Rust.catch_all("a: u8"), None);
    assert_eq!(
        Syntax::C.catch_all("const char *fmt, ...").as_deref(),
        Some("...")
    );
    assert_eq!(
        Syntax::Cpp.catch_all("int n, Ts... rest").as_deref(),
        Some("Ts... rest")
    );
    assert_eq!(
        Syntax::Swift.catch_all("_ xs: Int...").as_deref(),
        Some("_ xs: Int...")
    );
    assert_eq!(
        Syntax::Swift.catch_all("xs: Int..."),
        None,
        "a labeled variadic ends at the next label"
    );
}

#[test]
fn an_argument_is_added_at_the_end_of_whatever_was_there() {
    assert_eq!(with_argument("", "64"), "64");
    assert_eq!(with_argument("a, b", "64"), "a, b, 64");
    assert_eq!(with_argument("  a  ", "64"), "a, 64");
}
