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
fn tokens_preserve_byte_ranges_across_literals_and_crlf() {
    use Token::*;
    let text = "é 1..3 \"a\\\"b\" 'x' '\\n' 'life _ // skipped\r\n+";
    let actual: Vec<_> = tokens(text)
        .into_iter()
        .map(|(kind, start, end)| (kind, &text[start..end]))
        .collect();
    assert_eq!(
        actual,
        [
            (Word, "é"),
            (Number, "1"),
            (Punct, "."),
            (Punct, "."),
            (Number, "3"),
            (Str, "\"a\\\"b\""),
            (Char, "'x'"),
            (Char, "'\\n'"),
            (Word, "'life"),
            (Word, "_"),
            (Punct, "+")
        ]
    );
    assert_eq!(tokens("// no newline"), []);
    assert_eq!(tokens("\"open"), [(Str, 0, 5)]);
    assert_eq!(tokens("42_u8"), [(Number, 0, 5)]);
}

#[test]
fn field_paths_and_mentions_require_complete_identifiers() {
    for (expression, expected) in [
        ("((point.x.y))", true),
        ("point", true),
        ("pointless.x", false),
        ("point.", false),
        ("point..x", false),
        ("point.x()", false),
        ("point.é", true),
    ] {
        assert_eq!(
            is_field_path_of(expression, "point"),
            expected,
            "{expression}"
        );
    }
    for (text, expected) in [
        ("name", true),
        ("name + 1", true),
        ("éname", false),
        ("name_", false),
        ("(name)", true),
        ("names", false),
    ] {
        assert_eq!(mentions(text, "name"), expected, "{text}");
    }
}

#[test]
fn imported_and_duplicate_nominal_names_are_not_treated_as_unique() {
    let (structs, shadowed) = parse_unique_structs(
        "use source::{Remote, Item as Alias}; use source::*; struct Point { pub x: u32, y: bool } struct Twin {} struct Twin {} enum Choice {} union Storage { n: u32 } type Alias = Point; struct Generic<T> { value: T } struct Option {} // struct Fake {}\n",
    );
    assert_eq!(
        structs,
        BTreeMap::from([(
            "Point".into(),
            BTreeMap::from([("x".into(), "u32".into()), ("y".into(), "bool".into())])
        )])
    );
    assert_eq!(
        shadowed,
        ["Alias", "Item", "Option", "Remote", "Twin"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn let_statement_boundaries_ignore_nested_and_opaque_punctuation() {
    for (input, expected) in [
        (" x = call(1, [2]); trailing", Some((Some(3), 17))),
        (" x: u32;", Some((None, 7))),
        (" x = \";=\";", Some((Some(3), 9))),
        (" x = 1", None),
        (" x )", None),
    ] {
        assert_eq!(parse_let_stmt(input), expected, "{input}");
    }
}

#[test]
fn active_bindings_track_shadowing_scopes_and_patterns() {
    let code = "let mut outer: u32 = 1; let ref borrowed = outer; { let outer = 2; } let absent; let (left, right) = pair; for outer in values { let inside = 4; } if let Some(borrowed) = maybe {";
    let bindings = active_let_bindings(code);
    assert_eq!(
        bindings.keys().map(String::as_str).collect::<Vec<_>>(),
        ["outer"]
    );
    let outer = &bindings["outer"];
    assert_eq!(
        (
            outer.is_mut,
            outer.is_ref_pat,
            outer.explicit_ty.as_deref(),
            outer.rhs.as_str(),
            outer.stmt_start,
            outer.stmt_end
        ),
        (true, false, Some("u32"), "1", 0, 23)
    );
    let bindings = active_let_bindings("let ref mut item = value; let s = \";\";");
    assert_eq!(
        bindings.keys().map(String::as_str).collect::<Vec<_>>(),
        ["item", "s"]
    );
    let item = &bindings["item"];
    assert_eq!(
        (
            item.is_mut,
            item.is_ref_pat,
            item.explicit_ty.as_deref(),
            item.rhs.as_str()
        ),
        (true, true, None, "value")
    );
    assert_eq!(bindings["s"].rhs, "\";\"");
}
