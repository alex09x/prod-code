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

fn decl(text: &str) -> PolyglotDecl {
    PolyglotDecl {
        fn_name: "f".into(),
        name_start: text.find('f').unwrap(),
        open_paren: text.find('(').unwrap(),
        close_paren: text.find(')').unwrap(),
        body_open: text.find('{').unwrap_or(text.len()),
        body_close: text.len(),
        receiver: None,
        params: vec![],
        ret_span: None,
        is_async: false,
        async_keyword_span: None,
        visibility_span: None,
    }
}

#[test]
fn coverage_b2_return_edits() {
    let text = "  T f() {}";
    for (lang, expected) in [
        (Language::TypeScript, Some(": R ")),
        (Language::JavaScript, Some(": R ")),
        (Language::Python, Some(" -> R")),
        (Language::Swift, Some(" -> R ")),
        (Language::Go, Some(" R ")),
        (Language::Cpp, None),
        (Language::C, None),
        (Language::Java, None),
        (Language::Rust, None),
    ] {
        for span in [None, Some((0, 4))] {
            let mut d = decl(text);
            d.ret_span = span;
            let mut want = vec![(6, 6, "a: A".into())];
            if let Some(rendered) = expected {
                let (start, end) = span.unwrap_or((7, 7));
                want.push((start, end, rendered.into()));
            } else if span.is_some() && lang != Language::Rust {
                want.push((0, 4, "  R ".into()));
            }
            assert_eq!(
                build_decl_edits(
                    text,
                    &d,
                    "a: A",
                    &Modifiers {
                        returns: Some("R".into()),
                        ..Default::default()
                    },
                    lang
                ),
                want,
                "{lang:?} {span:?}"
            );
        }
    }
}

#[test]
fn coverage_b2_async_edits() {
    for (text, lang, expected) in [
        (
            "  function f() {}",
            Language::TypeScript,
            Some((2, 2, "async ")),
        ),
        (
            "// header\n  f() {}",
            Language::JavaScript,
            Some((12, 12, "async ")),
        ),
        ("  def f(): pass", Language::Python, Some((2, 2, "async "))),
        ("  f() {}", Language::Python, None),
        ("func f() {}", Language::Swift, Some((8, 8, " async"))),
        ("func f() {}", Language::Go, None),
    ] {
        let d = decl(text);
        let mut want = vec![(d.open_paren + 1, d.close_paren, String::new())];
        if let Some((s, e, t)) = expected {
            want.push((s, e, t.into()));
        }
        assert_eq!(
            build_decl_edits(
                text,
                &d,
                "",
                &Modifiers {
                    asyncness: Some(true),
                    ..Default::default()
                },
                lang
            ),
            want,
            "{text}"
        );
    }
    for (text, lang, span, expected) in [
        (
            "async function f() {}",
            Language::TypeScript,
            Some((0, 6)),
            Some((0, 6)),
        ),
        ("async f() {}", Language::JavaScript, None, None),
        (
            "  async def f(): pass",
            Language::Python,
            None,
            Some((2, 8)),
        ),
        ("def f(): pass", Language::Python, None, None),
        ("func f() async {}", Language::Swift, None, Some((9, 14))),
        ("func f() {}", Language::Swift, None, None),
        ("func f() {}", Language::Go, None, None),
    ] {
        let mut d = decl(text);
        d.is_async = true;
        d.async_keyword_span = span;
        let mut want = vec![(d.open_paren + 1, d.close_paren, String::new())];
        if let Some((s, e)) = expected {
            want.push((s, e, String::new()));
        }
        assert_eq!(
            build_decl_edits(
                text,
                &d,
                "",
                &Modifiers {
                    asyncness: Some(false),
                    ..Default::default()
                },
                lang
            ),
            want,
            "{text}"
        );
    }
}

#[test]
fn coverage_b2_visibility_edits() {
    for (text, lang, span, expected) in [
        (
            "public f() {}",
            Language::TypeScript,
            Some((0, 7)),
            Some((0, 7)),
        ),
        ("  f() {}", Language::JavaScript, None, Some((2, 2))),
        (
            "public func f() {}",
            Language::Swift,
            Some((0, 7)),
            Some((0, 7)),
        ),
        ("  final func f() {}", Language::Swift, None, Some((2, 8))),
        ("  f() {}", Language::Swift, None, None),
        ("  f() {}", Language::Python, None, None),
    ] {
        let mut d = decl(text);
        d.visibility_span = span;
        let mut want = vec![(d.open_paren + 1, d.close_paren, String::new())];
        if let Some((s, e)) = expected {
            want.push((s, e, "private ".into()));
        }
        assert_eq!(
            build_decl_edits(
                text,
                &d,
                "",
                &Modifiers {
                    visibility: Some("private".into()),
                    ..Default::default()
                },
                lang
            ),
            want
        );
    }
}
