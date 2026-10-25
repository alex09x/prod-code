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
fn classifies_expression_return_and_statement_outputs() {
    use OutputKind::*;
    let single = |name: &str, is_new| SingleVar {
        name: name.into(),
        is_new,
    };
    let cases = [
        ("", "", "", Expression("".into())),
        ("  x + 1\r\n", "", "", Expression("x + 1".into())),
        ("return x", "", "", EndsWithReturn),
        ("return\nx", "", "", EndsWithReturn),
        ("return;", "", "", Void),
        ("call();", "", "", Void),
        ("if (x) call()", "", "", Void),
        ("let", "", "", Void),
        ("let 1 = 2;", "", "", Void),
        ("let return = 2;", "return", "", Void),
        ("= x;", "x", "", Void),
        ("1 = 2;", "", "", Void),
        ("return = 2;", "return", "", EndsWithReturn),
        ("let x = 1;", "use(x)", "", single("x", true)),
        ("const x = 1;", "use(x)", "let x = 0", single("x", false)),
        ("var x = 1;", "use(xy)", "", Void),
        ("val x = 1;", "x", "", single("x", true)),
        ("x := 1;", "x", "", single("x", true)),
        (
            "x += 1; x -= 2; x *= 3; x /= 4;",
            "x",
            "x",
            single("x", false),
        ),
        (
            "let x = 1; let x = 2; y = 3;",
            "y + x",
            "",
            MultipleVars(vec!["x".into(), "y".into()]),
        ),
        ("let x = 1;", "", "", Void),
        ("\"x = 1\"", "x", "", Expression("\"x = 1\"".into())),
    ];
    for (selection, after, before, expected) in cases {
        assert_eq!(
            analyze_outputs(selection, after, before, Language::Rust),
            expected,
            "{selection:?}"
        );
    }
}

#[test]
fn reindents_preserving_relative_indentation_and_blank_lines() {
    let cases = [
        ("", 4, Language::Rust, ""),
        (
            "    α();\r\n      β();\r\n",
            2,
            Language::Rust,
            "  α();\n    β();",
        ),
        ("\n    x\n \n    y\n", 0, Language::Python, "\nx\n\ny"),
        (" \n\t", 4, Language::Rust, "\n\n"),
        ("x\n", 0, Language::Go, "\tx"),
        ("x\ny", 5, Language::Go, "\t\tx\n\t\ty"),
        ("\tx\n\t\ty", 4, Language::Rust, "\tx\n\t\ty"),
        ("    x\n\n", 2, Language::Rust, "  x\n\n"),
    ];
    for (body, indent, lang, expected) in cases {
        assert_eq!(reindent_body(body, indent, lang), expected, "{body:?}");
    }
}
