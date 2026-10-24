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
fn coverage_b2_signature_normalization_and_literals() {
    for (lines, lang, expected) in [
        (
            vec!["  def f( ", " x: int ) : # comment"],
            "python",
            "def f(x: int)",
        ),
        (
            vec!["fn f( x: T ) { // comment", "  "],
            "rust",
            "fn f(x: T)",
        ),
        (vec!["\tint  f();", "// only"], "cpp", "int f()"),
        (vec![], "go", ""),
    ] {
        assert_eq!(normalize_signature(&lines, lang), expected);
    }
    for (text, expected) in [
        ("x'", true),
        ("é'", true),
        ("\\n'", true),
        ("\\u{41}'", true),
        ("\\ ", false),
        ("\\unterminated", false),
        ("'", false),
        ("", false),
        ("a", false),
        (" ", false),
        ("life", false),
    ] {
        assert_eq!(is_rust_char_literal(text), expected, "{text}");
    }
}

#[test]
fn coverage_b2_signature_spans() {
    for (text, lang, line, expected) in [
        (
            "#[inline]\npub\nasync\nfn f<'a>(x: &'a [u8]) -> Vec<u8> { body() }",
            "rust",
            4,
            (1, 4, "#[inline] pub async fn f<'a>(x: &'a [u8]) -> Vec<u8>"),
        ),
        (
            "@decorator\ndef f(\n  x: str = '):', # comment\n) -> str: return x",
            "python",
            2,
            (1, 4, "@decorator def f(x: str = '):',) -> str"),
        ),
        (
            "template <class T>\nT f(T x);",
            "cpp",
            2,
            (1, 2, "template <class T> T f(T x)"),
        ),
        (
            "export\nfunction f(x = `)`, y = \"\\\"\") { return x; }",
            "typescript",
            2,
            (1, 2, "export function f(x = `)`, y = \"\\\"\")"),
        ),
        (
            "// comment\nfn f(x: char = '\\n', y: char = 'é') { }",
            "rust",
            2,
            (2, 2, "fn f(x: char = '\\n', y: char = 'é')"),
        ),
        (
            "fn f(\n// ignored )\n* ignored\nx: [T; 2]\n) // later\n",
            "rust",
            1,
            (1, 5, "fn f(* ignored x: [T; 2])"),
        ),
    ] {
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            extract_signature_span(&lines, line, "f", lang),
            Some((expected.0, expected.1, expected.2.into())),
            "{text}"
        );
    }
    assert_eq!(extract_signature_span(&[], 1, "f", "rust"), None);
    assert_eq!(extract_signature_span(&["fn f() {}"], 0, "f", "rust"), None);
    assert_eq!(extract_signature_span(&["fn f() {}"], 2, "f", "rust"), None);
}

#[test]
fn coverage_b2_find_declarations_across_languages() {
    for (lang, good, bad) in [
        (
            "rust",
            vec!["fn f() {}", "pub fn r#f() {}", "fn f<T>() {}"],
            vec![
                "fn foo() {}",
                "fn r#foo() {}",
                "let f = 1;",
                "fn other() {}",
            ],
        ),
        (
            "python",
            vec!["def f():", "async def f():", "def f"],
            vec!["def foo():", "async def foo():", "f()", "pass"],
        ),
        (
            "go",
            vec!["func f() {}", "func (r Receiver) f() {}", "func f"],
            vec![
                "func foo() {}",
                "func (r Receiver) foo() {}",
                "func (",
                "var f = 1",
            ],
        ),
        (
            "typescript",
            vec![
                "function f() {}",
                "const f = () => 1",
                "f() {}",
                "const f: T = x",
                "const f : T = x",
                "f<T>() {}",
            ],
            vec![
                "function foo() {}",
                "obj.f()",
                "_f() {}",
                "const other = 1",
                "function other() {}",
            ],
        ),
        (
            "swift",
            vec!["func f() {}", "f<T>() {}", "f() {}"],
            vec![
                "func foo() {}",
                "obj.f()",
                "_f() {}",
                "let x = 1",
                "func other() {}",
            ],
        ),
    ] {
        for line in good {
            let lines = ["// f()", "# f()", "* f()", line, "", line];
            assert_eq!(
                find_function_in_text(&lines, "f", lang, 6),
                Some(6),
                "{lang}: {line}"
            );
            assert_eq!(find_function_in_text(&lines, "f", lang, 5), Some(4));
        }
        for line in bad {
            assert_eq!(
                find_function_in_text(&[line], "f", lang, 1),
                None,
                "{lang}: {line}"
            );
        }
    }
}

#[test]
fn coverage_b2_map_new_lines_to_base() {
    let h = |start, added, removed| Hunk {
        start,
        added,
        removed,
    };
    for (hunks, line, expected) in [
        (vec![], 4, Some(4)),
        (vec![h(3, 2, 0)], 2, Some(2)),
        (vec![h(3, 2, 0)], 3, None),
        (vec![h(3, 2, 0)], 5, Some(3)),
        (vec![h(3, 3, 1)], 5, Some(3)),
        (vec![h(3, 1, 3)], 3, Some(3)),
        (vec![h(3, 0, 2)], 4, Some(6)),
        (vec![h(8, 1, 1), h(3, 2, 0)], 8, Some(6)),
    ] {
        assert_eq!(base_line_for_new_line(&hunks, line), expected);
    }
}
