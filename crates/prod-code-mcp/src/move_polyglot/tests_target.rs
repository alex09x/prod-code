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

use crate::parameter_object::Language;

use super::target::{
    check_target_collision, cpp_move_target_is_implementation, format_item_for_target,
    initial_file_header,
};

use Language::{C, Cpp, Go, Java, JavaScript, Python, Rust, Swift, TypeScript};

#[test]
fn format_item_for_target_table() {
    let cases: &[(Language, &str, &str)] = &[
        (Python, "def f():\n    pass", "def f():\n    pass"),
        (Go, "func f() {}", "func f() {}"),
        (TypeScript, "function f() {}\n", "export function f() {}"),
        (
            JavaScript,
            "async function f() {}",
            "export async function f() {}",
        ),
        (
            TypeScript,
            "/** doc */\n@decorator\nclass A {\n}",
            "/** doc */\n@decorator\nexport class A {\n}",
        ),
        (TypeScript, "export const x = 1;", "export const x = 1;"),
        (TypeScript, "  const x = 1;", "  export const x = 1;"),
        (
            TypeScript,
            "\n// note\nlet y = 2;",
            "\n// note\nexport let y = 2;",
        ),
        (
            TypeScript,
            "function a() {}\nfunction b() {}",
            "export function a() {}\nfunction b() {}",
        ),
        (TypeScript, "interface I {}", "export interface I {}"),
        (TypeScript, "type T = number;", "export type T = number;"),
        (TypeScript, "enum E {}", "export enum E {}"),
        (TypeScript, "var v = 1;", "export var v = 1;"),
        (TypeScript, "", ""),
    ];
    for (idx, (lang, item, expected)) in cases.iter().enumerate() {
        assert_eq!(format_item_for_target(item, *lang), *expected, "case {idx}");
    }
}

#[test]
fn collision_table() {
    // (language, target text, name, expected error)
    let cases: &[(Language, &str, &str, Option<&str>)] = &[
        (TypeScript, "", "foo", None),
        (TypeScript, "  \n", "foo", None),
        (
            TypeScript,
            "import a from \"a\";\nexport function foo() {}\n",
            "foo",
            Some("target already declares `foo` at line 2"),
        ),
        (TypeScript, "function foobar() {}\n", "foo", None),
        (TypeScript, "export enum foo {}\n", "foo", Some("line 1")),
        (TypeScript, "interface foo {}\n", "foo", Some("line 1")),
        (TypeScript, "export type foo = 1;\n", "foo", Some("line 1")),
        (TypeScript, "const foo = 1;\n", "foo", Some("line 1")),
        (TypeScript, "export class foo {}\n", "foo", Some("line 1")),
        (Python, "class K:\n    def foo(self): pass\n", "foo", None),
        (
            Python,
            "x = 1\nasync def foo():\n    pass\n",
            "foo",
            Some("line 2"),
        ),
        (Python, "def foo():\n    pass\n", "foo", Some("line 1")),
        (Go, "func foo() {}\n", "foo", Some("line 1")),
        (Go, "package p\nvar foo = 1\n", "foo", Some("line 2")),
        (Go, "package p\ntype foo struct{}\n", "foo", Some("line 2")),
        (Go, "const foo = 1\n", "foo", Some("line 1")),
        (Swift, "    func foo() {}\n", "foo", Some("line 1")),
        (Swift, "struct Other {}\n", "foo", None),
        (Cpp, "int foo(int);\n", "foo", Some("line 1")),
        (Cpp, "// foo is documented here\n", "foo", None),
        (C, "int bar(void);\n", "foo", None),
        (Java, "class foo {}\n", "foo", Some("line 1")),
        (Rust, "fn foo() {}\n", "foo", None),
    ];
    for (idx, (lang, text, name, expected)) in cases.iter().enumerate() {
        let got = check_target_collision(text, name, *lang);
        match expected {
            None => assert!(got.is_ok(), "case {idx}: {got:?}"),
            Some(fragment) => {
                let message = got
                    .expect_err(&format!("case {idx} must collide"))
                    .to_string();
                assert!(message.contains(fragment), "case {idx}: {message}");
                assert!(
                    message.starts_with("target already declares `foo`"),
                    "{message}"
                );
            }
        }
    }
}

#[test]
fn initial_header_for_non_go_languages() {
    assert_eq!(
        initial_file_header(Path::new("a.hpp"), Cpp),
        "#pragma once\n\n"
    );
    assert_eq!(initial_file_header(Path::new("a.h"), C), "#pragma once\n\n");
    assert_eq!(
        initial_file_header(Path::new("a.hxx"), Cpp),
        "#pragma once\n\n"
    );
    assert_eq!(initial_file_header(Path::new("a.cpp"), Cpp), "");
    assert_eq!(initial_file_header(Path::new("noext"), C), "");
    assert_eq!(initial_file_header(Path::new("a.ts"), TypeScript), "");
    assert_eq!(initial_file_header(Path::new("a.py"), Python), "");
}

#[test]
fn initial_go_header_follows_the_package_of_sibling_files() {
    let temp = tempfile::Builder::new().prefix("hdr").tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("a.go"), "// c\npackage alpha // trailing\n").unwrap();
    assert_eq!(
        initial_file_header(&dir.join("b.go"), Go),
        "package alpha\n\n"
    );
    // a test file target prefers the external test package when one exists
    std::fs::write(dir.join("x_test.go"), "package alpha_test\n").unwrap();
    assert_eq!(
        initial_file_header(&dir.join("new_test.go"), Go),
        "package alpha_test\n\n"
    );
    assert_eq!(
        initial_file_header(&dir.join("new.go"), Go),
        "package alpha\n\n"
    );
}

#[test]
fn initial_go_header_falls_back_to_test_package_and_directory_name() {
    let temp = tempfile::Builder::new().prefix("hdr").tempdir().unwrap();
    let only_tests = temp.path().join("only_tests");
    std::fs::create_dir_all(&only_tests).unwrap();
    std::fs::write(only_tests.join("x_test.go"), "package beta_test\n").unwrap();
    assert_eq!(
        initial_file_header(&only_tests.join("new.go"), Go),
        "package beta\n\n"
    );
    // a plain test package without a sibling regular file serves a test target
    assert_eq!(
        initial_file_header(&only_tests.join("other_test.go"), Go),
        "package beta_test\n\n"
    );

    let widgets = temp.path().join("widgets");
    std::fs::create_dir_all(&widgets).unwrap();
    assert_eq!(
        initial_file_header(&widgets.join("new.go"), Go),
        "package widgets\n\n"
    );

    let ghost = temp.path().join("ghost");
    assert_eq!(
        initial_file_header(&ghost.join("new.go"), Go),
        "package ghost\n\n"
    );
}

#[test]
fn cpp_implementation_target_detection() {
    for (lang, path, expected) in [
        (Cpp, "a.cpp", true),
        (Cpp, "a.cc", true),
        (Cpp, "a.cxx", true),
        (C, "a.c", true),
        (Cpp, "a.hpp", false),
        (C, "a.h", false),
        (Cpp, "noext", false),
        (Go, "a.cpp", false),
    ] {
        assert_eq!(
            cpp_move_target_is_implementation(lang, Path::new(path)),
            expected,
            "{path}"
        );
    }
}
