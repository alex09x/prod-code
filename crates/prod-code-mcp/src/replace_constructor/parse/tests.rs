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
fn coverage_balanced_lists_and_generics() {
    for (input, expected) in [
        ("", vec![]),
        (",, ", vec![]),
        (
            "a, Vec<(b, c)>, [d, e], { f, g }",
            vec!["a", "Vec<(b, c)>", "[d, e]", "{ f, g }"],
        ),
        (
            r#""a,\"b", 'c,\'d', tail,"#,
            vec![r#""a,\"b""#, r"'c,\'d'", "tail"],
        ),
        ("a, \"unterminated,", vec!["a", "\"unterminated,"]),
        ("a, 'unterminated,", vec!["a", "'unterminated,"]),
        ("α, β", vec!["α", "β"]),
    ] {
        assert_eq!(split_balanced_commas(input), expected);
    }
    for (text, start, expected) in [
        ("<> tail", 0, Some(("<>".into(), 2))),
        ("name  <A<B>, C> rest", 4, Some(("<A<B>, C>".into(), 15))),
        ("<T", 0, None),
        ("value", 0, None),
        ("", 0, None),
    ] {
        assert_eq!(extract_generics(text, start), expected);
    }
    for (text, expected) in [
        ("", false),
        ("1name", false),
        ("a-b", false),
        ("_x2", true),
        ("αβ", true),
    ] {
        assert_eq!(is_ident_str(text), expected);
    }
    assert_eq!(
        parse_struct_declaration("", "Item", "unknown")
            .err()
            .unwrap()
            .to_string(),
        "unsupported language `unknown` for replace_constructor"
    );
    for (lang, text) in [
        ("c", "struct Item { int x; };"),
        ("swift", "struct Item {\nvar x: Int\n}"),
    ] {
        let decl = parse_struct_declaration(text, "Item", lang).unwrap();
        assert_eq!((decl.name.as_str(), decl.language.as_str()), ("Item", lang));
        assert_eq!(decl.fields[0].name, "x");
    }
}
