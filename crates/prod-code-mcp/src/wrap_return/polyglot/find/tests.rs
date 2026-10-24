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
fn coverage_find_refusals_and_selected_lines() {
    for (text, line, symbol, error) in [
        (
            "",
            None,
            None,
            "could not determine function name to wrap return value for",
        ),
        (
            "function f() {}",
            Some(0),
            None,
            "could not determine function name to wrap return value for",
        ),
        (
            "function f() {}",
            Some(5),
            None,
            "could not determine function name to wrap return value for",
        ),
        (
            "// comment",
            Some(1),
            None,
            "could not determine function name to wrap return value for",
        ),
        (
            "function prefixf() {}\nfunction fextra() {}",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "// f()\nconst s = 'f()';\nimport { f } from 'x';\nexport { f };",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f<T {}",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f<T> value",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "const f = value => value",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "const f",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f(",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f()",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f() {",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
        (
            "function f();\nfunction g() {}",
            None,
            Some("f"),
            "function declaration `f` not found",
        ),
    ] {
        assert_eq!(
            find_polyglot_decl(text, Language::TypeScript, symbol, line)
                .err()
                .unwrap()
                .to_string(),
            error,
            "{text}"
        );
    }
    for prefix in [
        "const a = ",
        "return ",
        "throw ",
        "await ",
        "x + ",
        "x - ",
        "x * ",
        "x / ",
        "foo(",
        "foo, ",
        "key: ",
    ] {
        let text = format!("{prefix}f();\nfunction f(): number {{}}\n");
        let decl = find_polyglot_decl(&text, Language::TypeScript, Some("f"), None).unwrap();
        assert_eq!(decl.name_start, text.rfind("f()").unwrap());
        assert_eq!(decl.was, "number");
    }
    let text = "function f(): number {}\nfunction f(): string {}\n";
    let decl = find_polyglot_decl(text, Language::TypeScript, Some("Owner.f"), Some(2)).unwrap();
    assert_eq!(decl.was, "string");
    assert_eq!(decl.name_start, text.rfind("f()").unwrap());
    assert_eq!(
        find_polyglot_decl(text, Language::TypeScript, None, Some(2))
            .unwrap()
            .was,
        "string"
    );
    assert_eq!(
        find_polyglot_decl("def f()", Language::Python, Some("f"), None)
            .err()
            .unwrap()
            .to_string(),
        "function declaration `f` not found"
    );
}
