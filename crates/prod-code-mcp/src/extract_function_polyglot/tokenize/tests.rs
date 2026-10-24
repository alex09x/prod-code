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

mod vocabulary;

#[test]
fn exact_token_kinds_and_byte_spans() {
    use PolyTokenKind::*;
    let cases = [
        (Language::Python, "", vec![]),
        (Language::Python, " \r\n\t", vec![]),
        (
            Language::Python,
            "# comment\r\nα = 12",
            vec![
                (Word, 11, 13, "α"),
                (Punct, 14, 15, "="),
                (Number, 16, 18, "12"),
            ],
        ),
        (Language::Rust, "// tail", vec![]),
        (
            Language::Rust,
            "/* closed */x/* unclosed",
            vec![(Word, 12, 13, "x")],
        ),
        (
            Language::Go,
            "12..34 0xff 1.25 1_000",
            vec![
                (Number, 0, 2, "12"),
                (Punct, 2, 3, "."),
                (Punct, 3, 4, "."),
                (Number, 4, 6, "34"),
                (Number, 7, 11, "0xff"),
                (Number, 12, 16, "1.25"),
                (Number, 17, 22, "1_000"),
            ],
        ),
        (Language::Python, "'é'", vec![(Str, 0, 4, "'é'")]),
        (Language::Rust, "'é'", vec![(Char, 0, 4, "'é'")]),
        (Language::Cpp, "'\\''", vec![(Char, 0, 4, "'\\''")]),
        (Language::C, "'x", vec![(Char, 0, 2, "'x")]),
        (Language::Zig, "'\\", vec![(Char, 0, 2, "'\\")]),
        (Language::JavaScript, "`é`", vec![(Str, 0, 4, "`é`")]),
        (
            Language::Swift,
            "\"a\\\"b\"",
            vec![(Str, 0, 6, "\"a\\\"b\"")],
        ),
        (
            Language::TypeScript,
            "\"\\é\"",
            vec![(Str, 0, 5, "\"\\é\"")],
        ),
        (Language::Java, "\"tail\\", vec![(Str, 0, 6, "\"tail\\")]),
        (Language::Kotlin, "\"é", vec![(Str, 0, 3, "\"é")]),
        (
            Language::Csharp,
            "é_2 €!",
            vec![(Word, 0, 4, "é_2"), (Punct, 5, 8, "€"), (Punct, 8, 9, "!")],
        ),
        (Language::Python, "# eof", vec![]),
        (Language::Go, "// skip\nx", vec![(Word, 8, 9, "x")]),
    ];
    for (lang, input, expected) in cases {
        let actual: Vec<_> = tokenize_polyglot(input, lang)
            .into_iter()
            .map(|t| (t.kind, t.start, t.end, t.text))
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(k, s, e, t)| (k, s, e, t.to_string()))
            .collect();
        assert_eq!(actual, expected, "{lang:?}: {input:?}");
    }
}

#[test]
fn operators_have_exact_spans() {
    let operators = [
        "->", "=>", "::", ":=", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=",
        "/=", "+", "(", ")", "€",
    ];
    for operator in operators {
        assert_eq!(
            tokenize_polyglot(operator, Language::Rust),
            vec![PolyToken {
                kind: PolyTokenKind::Punct,
                start: 0,
                end: operator.len(),
                text: operator.into()
            }]
        );
    }
}
