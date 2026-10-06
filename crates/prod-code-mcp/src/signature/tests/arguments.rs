/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::{blank_comments, call_arguments};
use crate::signature::effects::classify::classify_arg;
use crate::signature::types::{ArgKind, FIELD_DEREF, ParamFacts, REF_COERCION, UNCONFIRMED_TYPE};

/// #442: what the text of an argument, and what the analyzer confirmed about the parameter's
/// type, say about evaluating it.
#[test]
fn arguments_are_told_apart_by_what_evaluating_them_can_do() {
    let free = ParamFacts {
        drop_free: true,
        coercion_free: true,
        ..Default::default()
    };
    let reference = ParamFacts {
        drop_free: true,
        reference: true,
        ..Default::default()
    };
    let unknown = ParamFacts::default();
    for literal in [
        "1",
        "-2",
        "0x1F_u8",
        "1.5f32",
        "true",
        "\"a, b\"",
        "b\"x\"",
        "r#\"q\"#",
        "'c'",
        "'\\n'",
        "b'x'",
        "&5",
        "&\"lit\"",
        "/* c */ 7",
        "\"/* not a comment */\"",
    ] {
        for facts in [&free, &reference, &unknown] {
            assert_eq!(classify_arg(literal, facts), ArgKind::Literal, "{literal}");
        }
    }
    for place in [
        "x",
        "crate::LIMIT",
        "&k",
        "&mut buf",
        "& mut buf",
        "n as u64",
        "x /* a, /* b, */ c */",
        "/* a */ x // b\n",
    ] {
        assert_eq!(classify_arg(place, &free), ArgKind::Place, "{place}");
    }
    // A cast makes a built-in value that no `Deref` converts.
    assert_eq!(classify_arg("n as u64", &unknown), ArgKind::Place);
    // A field read can go through `Deref`, whatever the parameter.
    for field in ["self.a.0", "&self.items", "a.n", "&mut w.buf", "a /* */ .n"] {
        let kind = classify_arg(field, &free);
        assert!(
            kind == ArgKind::Unproven(FIELD_DEREF) || kind == ArgKind::Effectful,
            "{field}: {kind:?}"
        );
    }
    assert_eq!(classify_arg("a.n", &free), ArgKind::Unproven(FIELD_DEREF));
    // Passed for a reference, a value or a reference to it can be converted by `Deref`; for
    // a type the analyzer did not describe, by whatever that type turns out to be.
    for arg in ["k", "&owned", "&mut buf", "crate::LIMIT"] {
        assert_eq!(
            classify_arg(arg, &reference),
            ArgKind::Unproven(REF_COERCION),
            "{arg}"
        );
        assert_eq!(
            classify_arg(arg, &unknown),
            ArgKind::Unproven(UNCONFIRMED_TYPE),
            "{arg}"
        );
    }
    for effect in [
        "x /* /* */",
        "/* unclosed x",
        "mark() /* /* */ */",
        "/* x, /* y */ */ mark()",
        "mark(\"a\")",
        "x.len()",
        "v[0]",
        "a + 1",
        "*r",
        "f()?",
        "fut.await",
        "vec![1]",
        "{ x }",
        "\"a\" \"b\"",
        "1..2",
        "\"a\".len()",
        "Noisy(\"x\")",
        "|x| x",
        "&mut make()",
    ] {
        assert_eq!(classify_arg(effect, &free), ArgKind::Effectful, "{effect}");
    }
}

/// #442: a comment ends where its nesting does, so a comma, a quote or a call inside a
/// nested comment is not an argument, and an argument after it is not hidden in it.
#[test]
fn nested_block_comments_are_one_comment() {
    let text = "join(/* a, /* b, \" */ c, */ x, /* ' */ mark(\"y\"))";
    assert_eq!(
        call_arguments(text, 0).unwrap().unwrap(),
        ["/* a, /* b, \" */ c, */ x", "/* ' */ mark(\"y\")"]
    );
    let blanked = blank_comments("/* a /* b */ c */ x").unwrap();
    assert_eq!(blanked.trim(), "x");
    assert_eq!(blanked.len(), "/* a /* b */ c */ x".len());
    assert_eq!(
        blank_comments("\"/*\" /* é */ y").unwrap(),
        format!("\"/*\" {}y", " ".repeat("/* é */ ".len()))
    );
    assert!(call_arguments("join(x /* /* */, y)", 0).is_err());
    assert!(blank_comments("x /* /* */").is_none());
}

#[test]
fn call_arguments_are_split_at_their_own_commas_only() {
    let text = "let n = join(\"a, (b\", ',', Vec::<(u8, u8)>::new(), f(x, y), r#\"q\"#, 'a', /* c, */ z);\n";
    assert_eq!(
        call_arguments(text, text.find("join").unwrap())
            .unwrap()
            .unwrap(),
        [
            "\"a, (b\"",
            "','",
            "Vec::<(u8, u8)>::new()",
            "f(x, y)",
            "r#\"q\"#",
            "'a'",
            "/* c, */ z"
        ]
    );
    let method = "    make().send::<u8>(a, b)\n";
    assert_eq!(
        call_arguments(method, method.find("send").unwrap())
            .unwrap()
            .unwrap(),
        ["a", "b"]
    );
    // A use as a value and an import are not calls.
    let value = "let g = join;\nmap(join)\n";
    assert_eq!(
        call_arguments(value, value.find("join").unwrap()).unwrap(),
        None
    );
    assert_eq!(
        call_arguments(value, value.rfind("join").unwrap()).unwrap(),
        None
    );
    assert_eq!(
        call_arguments("join(\n    a,\n    b,\n)", 0)
            .unwrap()
            .unwrap(),
        ["a", "b"]
    );
    assert!(call_arguments("join()", 0).unwrap().unwrap().is_empty());
    assert!(call_arguments("join(a, b", 0).is_err());
}
