/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The text-level questions the polyglot signature change asks of a source file: whether an
//! argument can be dropped, whether a position is a comment or an import, and which name a
//! declaration line gives.

use prod_code_mcp::parameter_object::Language;
use prod_code_mcp::signature_polyglot::syntax::{
    display, extract_decl_name_from_line, find_python_body_close, is_ident,
    is_import_or_export_context, is_in_comment, is_side_effect_free_argument, is_word_used,
    one_based_lsp_position,
};
use std::path::Path;

#[test]
fn only_arguments_without_effects_may_be_dropped() {
    for (argument, lang, expected) in [
        ("x", Language::TypeScript, true),
        ("foo()", Language::TypeScript, false),
        ("a.b", Language::TypeScript, false),
        ("", Language::TypeScript, false),
        ("  ", Language::TypeScript, false),
        ("null", Language::TypeScript, true),
        ("nil", Language::Go, true),
        ("nullptr", Language::Cpp, true),
        ("true", Language::Java, true),
        ("-1", Language::TypeScript, true),
        ("+2.5e-3", Language::TypeScript, true),
        ("0x1F_u8", Language::Rust, true),
        ("1 + f()", Language::TypeScript, false),
        ("'abc'", Language::TypeScript, true),
        ("\"a b\"", Language::Go, true),
        ("\"a\nb\"", Language::Go, false),
        ("'abc", Language::TypeScript, false),
        ("\"x'", Language::TypeScript, false),
        // A lone quote opens a string that nothing closes.
        ("'", Language::TypeScript, false),
        ("\"", Language::Cpp, false),
        // A keyword argument is judged by its value.
        ("x=5", Language::Python, true),
        ("name = foo()", Language::Python, false),
        ("flag=True", Language::Python, true),
        ("None", Language::Python, true),
        ("label: value", Language::Swift, true),
        ("label: f()", Language::Swift, false),
        ("label: -3", Language::Swift, true),
        ("1.5", Language::Swift, true),
        ("a.b: c", Language::Swift, false),
    ] {
        assert_eq!(
            is_side_effect_free_argument(argument, lang),
            expected,
            "{argument:?} in {lang:?}"
        );
    }
}

#[test]
fn positions_and_paths_are_shown_as_the_editor_counts_them() {
    let text = "ab\nc😀d";
    for (offset, expected) in [
        (0, (1, 1)),
        (2, (1, 3)),
        (3, (2, 1)),
        (8, (2, 4)),
        (9, (2, 5)),
    ] {
        assert_eq!(one_based_lsp_position(text, offset), expected, "{offset}");
    }
    let root = Path::new("/w");
    assert_eq!(display(root, Path::new("/w/a/b.py")), "a/b.py");
    assert_eq!(display(root, Path::new("/x/b.py")), "/x/b.py");
    assert!(is_ident('é') && is_ident('_') && !is_ident(' '));
}

#[test]
fn comments_and_imports_are_recognised_per_language() {
    let after = |text: &str, marker: &str| text.find(marker).unwrap();
    for (text, marker, lang, expected) in [
        ("x = 1  # note", "note", Language::Python, true),
        ("# note", "note", Language::Python, true),
        ("x = note", "note", Language::Python, false),
        ("// note", "note", Language::Go, true),
        ("  /* note", "note", Language::Cpp, true),
        (" * note", "note", Language::Java, true),
        ("call(); // note", "note", Language::TypeScript, true),
        ("call(note);", "note", Language::TypeScript, false),
    ] {
        assert_eq!(
            is_in_comment(text, after(text, marker), lang),
            expected,
            "{text:?}"
        );
    }
    for (text, marker, lang, expected) in [
        ("import os", "os", Language::Python, true),
        ("from a import b", "a import", Language::Python, true),
        ("x = os", "os", Language::Python, false),
        ("import { a } from 'x';", "a }", Language::TypeScript, true),
        ("const m = require('y');", "m =", Language::JavaScript, true),
        (
            "export { a } from './z';",
            "a }",
            Language::TypeScript,
            true,
        ),
        (
            "import {\n  a,\n  b,\n} from './z';",
            "b,",
            Language::TypeScript,
            true,
        ),
        (
            "import { a } from 'x';\nconst z = a;",
            "z =",
            Language::TypeScript,
            false,
        ),
        ("#include <x>", "x>", Language::Cpp, true),
        ("using namespace std;", "std", Language::C, true),
        ("int x;", "x;", Language::Cpp, false),
        ("import Foundation", "Foundation", Language::Swift, true),
        ("import \"fmt\"", "fmt", Language::Go, true),
        ("use std::io;", "io", Language::Rust, true),
        ("import a.b;", "b;", Language::Java, true),
        ("package p;", "p;", Language::Java, true),
        ("let q = 1;", "q", Language::Rust, false),
    ] {
        assert_eq!(
            is_import_or_export_context(text, after(text, marker), lang),
            expected,
            "{text:?} in {lang:?}"
        );
    }
}

#[test]
fn a_word_counts_as_used_only_as_a_whole_identifier_outside_comments() {
    for (body, word, expected) in [
        ("let x = foo + 1", "foo", true),
        ("foo", "foo", true),
        ("a.foo", "foo", true),
        ("xfoo foo2 _foo", "foo", false),
        ("// foo\n", "foo", false),
        ("# foo", "foo", false),
        ("a // foo\nfoo", "foo", true),
        ("", "foo", false),
    ] {
        assert_eq!(is_word_used(body, word), expected, "{body:?}");
    }
}

#[test]
fn a_python_body_ends_at_the_first_line_that_is_not_indented_deeper() {
    let flat = "def f(x):\n    a = 1\n\n    # note\n    b = 2\ndef g():\n    pass\n";
    let colon = flat.find(':').unwrap();
    assert_eq!(
        find_python_body_close(flat, 0, colon),
        flat.find("def g").unwrap()
    );
    // A body that runs to the end of the file closes there.
    let last = "def f(x):\n    a = 1\n    b = 2\n";
    assert_eq!(
        find_python_body_close(last, 0, last.find(':').unwrap()),
        last.len()
    );
    // A method closes at the next member, not at the end of its class.
    let class = "class A:\n    def m(self):\n        x = 1\n    def n(self):\n        pass\n";
    let def = class.find("def m").unwrap();
    let colon = def + class[def..].find(':').unwrap();
    assert_eq!(
        find_python_body_close(class, def, colon),
        class.find("    def n").unwrap()
    );
}

#[test]
fn a_declaration_line_names_its_function_in_each_language() {
    for (line, lang, expected) in [
        ("    def run(self):", Language::Python, Some("run")),
        ("async def go():", Language::Python, Some("go")),
        ("x = 1", Language::Python, None),
        ("def broken", Language::Python, None),
        ("func add(a: Int) -> Int {", Language::Swift, Some("add")),
        ("func add<T>(a: T) -> T {", Language::Swift, Some("add")),
        ("let f = 1", Language::Swift, None),
        (
            "export function run(x) {",
            Language::TypeScript,
            Some("run"),
        ),
        (
            "function id<T>(x: T): T {",
            Language::TypeScript,
            Some("id"),
        ),
        (
            "  async method(a, b) {",
            Language::JavaScript,
            Some("method"),
        ),
        ("if (x) {", Language::TypeScript, None),
        ("const y = 1;", Language::TypeScript, None),
        (
            "func (s *Srv) Handle(w int) {",
            Language::Go,
            Some("Handle"),
        ),
        ("func Free(x int) {", Language::Go, Some("Free")),
        ("func Map[T any](x T) T {", Language::Go, Some("Map")),
        ("x := 1", Language::Go, None),
        ("int Widget::run(int a) const {", Language::Cpp, Some("run")),
        ("static int* make(int x)", Language::C, Some("make")),
        ("public void close() {", Language::Java, Some("close")),
        ("while (x) {", Language::Cpp, None),
        ("x;", Language::Cpp, None),
        ("fn main() {}", Language::Rust, None),
    ] {
        assert_eq!(
            extract_decl_name_from_line(line, lang).as_deref(),
            expected,
            "{line:?} in {lang:?}"
        );
    }
}
