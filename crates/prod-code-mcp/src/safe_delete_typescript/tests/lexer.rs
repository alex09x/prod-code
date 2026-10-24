/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::super::lexer::{
    advance, ascii_identifier, ascii_identifier_end, keyword_at, matching, opaque_end, pair,
    skip_trivia,
};

fn err<T>(r: anyhow::Result<T>) -> String {
    r.err().expect("an error").to_string()
}

#[test]
fn matching_finds_the_closer_and_refuses_what_it_cannot_scan() {
    let ok = [
        ("(a(b)c)", 0, 6),
        ("{ [ ( ) ] }", 0, 10),
        ("x(')' /* ) */ \")\" // )\n)", 1, 23),
        ("(`a)`)", 0, 5),
        ("(é)", 0, 3),
    ];
    for (text, open, want) in ok {
        assert_eq!(matching(text, open).unwrap(), want, "{text:?}");
    }
    let refused = [
        ("x(", 9, "a delimiter is outside the source"),
        ("x(a)", 0, "expected an opening delimiter"),
        (
            "(a",
            0,
            "an opening delimiter in the declaration is not closed",
        ),
        ("(a]", 0, "a delimiter is mismatched in the declaration"),
        (
            "(a / b)",
            0,
            "regular-expression and division syntax is outside the safe-delete subset",
        ),
        (
            "(`${a}`)",
            0,
            "template expressions are outside the safe-delete subset",
        ),
        ("('a)", 0, "a quoted literal is not terminated"),
    ];
    for (text, open, want) in refused {
        assert_eq!(err(matching(text, open)), want, "{text:?}");
    }
}

#[test]
fn opaque_spans_end_after_their_closer_and_unterminated_ones_are_errors() {
    let ok: &[(&str, usize, Option<usize>)] = &[
        ("'ab' x", 0, Some(4)),
        ("\"a\\\"b\"", 0, Some(6)),
        ("`ab`x", 0, Some(4)),
        ("`a$`", 0, Some(4)),
        ("// hi\nrest", 0, Some(5)),
        ("// hi", 0, Some(5)),
        ("/* a */x", 0, Some(7)),
        ("/**/", 0, Some(4)),
        ("abc", 0, None),
        ("a/b", 1, None),
        ("/", 0, None),
        ("ab", 5, None),
    ];
    for (text, start, want) in ok {
        assert_eq!(opaque_end(text, *start).unwrap(), *want, "{text:?}@{start}");
    }
    let refused = [
        ("'ab", "a quoted literal is not terminated"),
        ("'a\nb'", "a quoted literal crosses a line without closing"),
        ("'a\rb'", "a quoted literal crosses a line without closing"),
        ("'a\\", "a quoted escape is truncated"),
        ("`ab", "a template literal is not terminated"),
        ("`a\\", "a template escape is truncated"),
        (
            "`${x}`",
            "template expressions are outside the safe-delete subset",
        ),
        ("/* a", "a block comment is not terminated"),
    ];
    for (text, want) in refused {
        assert_eq!(err(opaque_end(text, 0)), want, "{text:?}");
    }
}

#[test]
fn trivia_is_whitespace_and_comments_only() {
    let text = "  /* c */ // d\n  x";
    assert_eq!(skip_trivia(text, 0).unwrap(), text.find('x').unwrap());
    assert_eq!(skip_trivia("x", 0).unwrap(), 0);
    assert_eq!(skip_trivia("  ", 0).unwrap(), 2);
    assert_eq!(
        skip_trivia("  'a'", 0).unwrap(),
        2,
        "a string is not trivia"
    );
    assert_eq!(skip_trivia("a", 7).unwrap(), 7);
    assert_eq!(
        err(skip_trivia(" /* open", 0)),
        "a block comment is not terminated"
    );
}

#[test]
fn keywords_need_token_boundaries_on_both_sides() {
    let cases = [
        ("import x", 0, true),
        ("a import", 2, true),
        ("imports", 0, false),
        ("ximport", 1, false),
        ("im", 0, false),
        ("éimport", 1, false),
        ("éimport", 2, false),
        ("$import", 1, false),
        ("import", 0, true),
    ];
    for (text, start, want) in cases {
        assert_eq!(keyword_at(text, start, "import"), want, "{text:?}@{start}");
    }
}

#[test]
fn identifiers_are_ascii_and_not_reserved() {
    let ends: &[(&str, usize, Option<usize>)] = &[
        ("abc1 x", 0, Some(4)),
        ("$_a", 0, Some(3)),
        ("1a", 0, None),
        ("", 0, None),
        ("a", 5, None),
        ("é", 0, None),
        ("x_y-z", 0, Some(3)),
    ];
    for (text, start, want) in ends {
        assert_eq!(ascii_identifier_end(text, *start), *want, "{text:?}");
    }
    for name in ["foo", "_a$1", "classes", "Await"] {
        assert!(ascii_identifier(name), "{name}");
    }
    for name in ["", "1a", "a-b", "é", "a b"] {
        assert!(!ascii_identifier(name), "{name:?}");
    }
    for word in [
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ] {
        assert!(!ascii_identifier(word), "{word}");
    }
}

#[test]
fn pairs_and_advance_follow_delimiters_and_characters() {
    for (o, c, want) in [
        (b'(', b')', true),
        (b'[', b']', true),
        (b'{', b'}', true),
        (b'(', b']', false),
        (b'{', b')', false),
    ] {
        assert_eq!(pair(o, c), want);
    }
    assert_eq!(advance("é", 0), 2);
    assert_eq!(advance("ab", 1), 2);
    assert_eq!(advance("a", 1), 2);
}
