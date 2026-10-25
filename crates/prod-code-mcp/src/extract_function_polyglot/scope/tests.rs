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

mod inputs;
mod python;

#[test]
fn balanced_delimiters_ignore_quoted_and_escaped_characters() {
    for (text, expected) in [
        ("", true),
        ("α([{}])", true),
        ("([)]", false),
        (")", false),
        ("]", false),
        ("}", false),
        ("(", false),
        ("'('", true),
        ("\"[\"", true),
        ("`{`", true),
        ("\"a\\\"b\"", true),
        ("'\\''", true),
        ("\"tail\\", false),
        ("'open", false),
        ("`open", false),
        ("{\r\n[1, 2]\r\n}", true),
    ] {
        assert_eq!(is_balanced(text), expected, "{text:?}");
    }
}

#[test]
fn expression_boundaries_require_both_sides() {
    for left in ["", "(", "[", "{", ",", ":", "=", ";", "return ", "=> "] {
        for right in [
            "", ")", "]", "}", ",", ";", ":", "\n", "\r\n", " // note", "\t# note",
        ] {
            let text = format!("{left}α + 1{right}");
            assert!(
                has_complete_expression_boundaries(&text, left.len(), left.len() + "α + 1".len()),
                "{text:?}"
            );
        }
    }
    for (text, start, end) in [
        ("x + y", 4, 5),
        ("x + y", 0, 1),
        ("α + β", 0, 2),
        ("value.member", 6, 12),
    ] {
        assert!(
            !has_complete_expression_boundaries(text, start, end),
            "{text:?}"
        );
    }
}

#[test]
fn finds_braced_functions_return_types_and_methods() {
    let cases = [
        (Language::Rust, "fn compute() -> i32", Some("i32")),
        (
            Language::TypeScript,
            "function compute(): number",
            Some("number"),
        ),
        (Language::Go, "func compute() int", Some("int")),
        (Language::Java, "public int compute()", Some("int")),
        (Language::Cpp, "int compute()", Some("int")),
        (Language::C, "int compute()", Some("int")),
        (Language::Csharp, "public int compute()", Some("int")),
        (Language::JavaScript, "function compute()", None),
        (Language::Swift, "func compute() -> Int", Some("Int")),
        (Language::Kotlin, "fun compute(): Int", Some("Int")),
        (Language::Zig, "fn compute() u32", Some("u32")),
        (Language::Rust, "fn compute() ->", None),
        (Language::TypeScript, "function compute():", None),
        (Language::Cpp, "static compute()", None),
    ];
    for (lang, head, ret) in cases {
        for indent in ["", "    "] {
            let text = format!("// lead\n{indent}{head} {{\n{indent}    value();\n{indent}}}\nend");
            let start = text.find("value").unwrap();
            assert_eq!(
                find_enclosing_scope(&text, lang, start, start + 5),
                (
                    8,
                    text.rfind('}').unwrap() + 1,
                    Some("compute".into()),
                    ret.map(str::to_string),
                    !indent.is_empty(),
                    indent.into()
                ),
                "{text}"
            );
        }
    }
}

#[test]
fn skips_control_blocks_closed_blocks_and_nonfunction_braces() {
    for control in [
        "if",
        "for",
        "while",
        "switch",
        "catch",
        "synchronized",
        "with",
        "lock",
        "using",
        "try",
        "else",
        "do",
    ] {
        let text =
            format!("fn compute() {{\n    {control} (ready) {{\n        value();\n    }}\n}}");
        let start = text.find("value").unwrap();
        assert_eq!(
            find_enclosing_scope(&text, Language::Rust, start, start + 5),
            (
                0,
                text.len(),
                Some("compute".into()),
                None,
                false,
                "".into()
            )
        );
    }
    let text = "fn compute() {\n    {}\n    value();\n}";
    let start = text.find("value").unwrap();
    assert_eq!(
        find_enclosing_scope(text, Language::Rust, start, start + 5),
        (
            0,
            text.len(),
            Some("compute".into()),
            None,
            false,
            "".into()
        )
    );
    for text in ["value", "{ value }", "( ) { value }", "{ value"] {
        let start = text.find("value").unwrap();
        assert_eq!(
            find_enclosing_scope(text, Language::Rust, start, start + 5),
            (0, text.len(), None, None, false, "".into())
        );
    }
}
