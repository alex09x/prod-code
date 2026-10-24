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

fn errors(text: &str, svg: bool) -> Vec<(u32, u32, String)> {
    let mut p = XmlParser::new("document.xml", text, svg);
    p.parse();
    for d in &p.diagnostics {
        assert_eq!(d.severity, "error");
        assert_eq!(
            d.code.as_deref(),
            Some(if svg { "svg-syntax" } else { "xml-syntax" })
        );
    }
    p.diagnostics
        .into_iter()
        .map(|d| (d.line, d.col, d.message))
        .collect()
}

#[test]
fn valid_markup_and_declarations() {
    for text in [
        "<root/>",
        "<?xml version='1.0'?><root><!--ok--><![CDATA[<>&]]></root>",
        "<!DOCTYPE root [<!ENTITY a 'a>b'>]><root/>",
        "<!doctype root SYSTEM \"a>b\"><root/>",
        "<root a='one' b=\"two\"><ns:child-x.y>α\r\n&amp;&lt;&gt;&apos;&quot;&#10;&#xAf;</ns:child-x.y></root>",
    ] {
        assert_eq!(errors(text, false), vec![], "{text}");
    }
    assert_eq!(errors("<svg/>", true), vec![]);
}

#[test]
fn malformed_constructs_have_exact_diagnostics() {
    for (text, svg, expected) in [
        (
            "",
            false,
            vec![(1, 1, "empty XML document; expected root element")],
        ),
        (
            " ",
            true,
            vec![(1, 1, "empty SVG document; expected `<svg>` root element")],
        ),
        ("<!--x", false, vec![(1, 1, "unclosed XML comment `<!--`")]),
        (
            "<![CDATA[x",
            false,
            vec![(1, 1, "unclosed CDATA section `<![CDATA[`")],
        ),
        (
            "<?xml",
            false,
            vec![(
                1,
                1,
                "unclosed XML processing instruction or declaration `<?`",
            )],
        ),
        (
            "<!DOCTYPE root [",
            false,
            vec![(1, 1, "unclosed DOCTYPE declaration")],
        ),
        (
            "</>",
            false,
            vec![(1, 1, "expected element name in closing tag `</...>`")],
        ),
        (
            "< />",
            false,
            vec![(1, 1, "expected XML element name after `<`")],
        ),
        (
            "</r>",
            false,
            vec![(
                1,
                1,
                "unexpected closing tag `</r>` without matching opening tag",
            )],
        ),
        (
            "<r></x>",
            false,
            vec![(
                1,
                4,
                "mismatched closing tag `</x>`; expected `</r>` (opened at line 1, col 1)",
            )],
        ),
        ("<r></r x>", false, vec![(1, 8, "expected `>` after `</r`")]),
        (
            "<r/><x/>",
            false,
            vec![(
                1,
                5,
                "multiple root elements in XML document; extra root `<x>`",
            )],
        ),
        (
            "<r/>",
            true,
            vec![(
                1,
                1,
                "SVG document root element must be `<svg>`, found `<r>`",
            )],
        ),
        (
            "<r",
            false,
            vec![
                (1, 1, "unclosed opening tag `<r>` at end of file"),
                (1, 1, "unclosed XML tag `<r>` opened at line 1, col 1"),
            ],
        ),
        (
            "<r ?/>",
            false,
            vec![
                (1, 4, "expected attribute name or `>` in tag `<r>`"),
                (1, 1, "unclosed XML tag `<r>` opened at line 1, col 1"),
            ],
        ),
        (
            "<r a='1' a='2'/>",
            false,
            vec![(1, 10, "duplicate attribute `a` in tag `<r>`")],
        ),
        (
            "<r a 'v'/>",
            false,
            vec![(1, 6, "expected `=` after attribute `a` in tag `<r>`")],
        ),
        (
            "<r a='<'>",
            false,
            vec![
                (
                    1,
                    7,
                    "unescaped `<` inside attribute value for `a` in tag `<r>`",
                ),
                (1, 1, "unclosed XML tag `<r>` opened at line 1, col 1"),
            ],
        ),
        (
            "<r a='x",
            false,
            vec![
                (1, 6, "unclosed quote for attribute `a` in tag `<r>`"),
                (1, 1, "unclosed XML tag `<r>` opened at line 1, col 1"),
            ],
        ),
        (
            "<r/>x",
            false,
            vec![(
                1,
                5,
                "content is not allowed in trailing section after root element",
            )],
        ),
        (
            "<r>&bad;</r>",
            false,
            vec![(1, 4, "unknown or unsupported XML entity `&bad;`")],
        ),
        (
            "<r>& </r>",
            false,
            vec![(1, 4, "unescaped `&` in XML text; use `&amp;` instead")],
        ),
        (
            "<r>&abcdefghijkl</r>",
            false,
            vec![(1, 4, "unescaped `&` in XML text; use `&amp;` instead")],
        ),
    ] {
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(l, c, m)| (l, c, m.to_string()))
            .collect();
        assert_eq!(errors(text, svg), expected, "{text}");
    }
}

#[test]
fn diagnostic_limit_and_unquoted_values() {
    assert_eq!(
        errors("<r a=/>", false),
        vec![(
            1,
            6,
            "expected quoted value ('\"' or '\'') for attribute `a` in tag `<r>`".into()
        )]
    );
    let expected: Vec<_> = (5..15)
        .map(|col| {
            (
                1,
                col,
                "content is not allowed in trailing section after root element".into(),
            )
        })
        .collect();
    assert_eq!(errors("<r/>xxxxxxxxxxxx", false), expected);
}
