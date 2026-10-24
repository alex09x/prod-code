/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use super::super::super::types::{FieldDecl, InstantiationSite, StructDecl};
use super::{go_literal_values, go_zero_value, is_pure_reorder_value};

#[test]
fn pure_reorder_values() {
    let cases: &[(&str, bool)] = &[
        ("true", true),
        ("  false  ", true),
        ("nil", true),
        ("None", true),
        ("null", true),
        ("nullptr", true),
        ("foo", true),
        ("a::b", true),
        ("a::", false),
        ("'x'", true),
        ("\"s\"", true),
        ("`t`", true),
        ("\"a${b}\"", false),
        ("`a${b}`", false),
        ("x.into()", true),
        ("name.to_string()", true),
        ("\"a\".to_owned()", true),
        ("v.clone()", true),
        ("foo().clone()", false),
        ("42", true),
        ("-3.5", true),
        ("+7", true),
        ("0xFF", true),
        ("1_000u64", true),
        ("-", false),
        ("a + b", false),
        ("f(x)", false),
        ("x.y", false),
    ];
    for (value, expected) in cases {
        assert_eq!(is_pure_reorder_value(value), *expected, "{value:?}");
    }
}

#[test]
fn go_zero_values() {
    let cases: &[(&str, &str)] = &[
        ("bool", "false"),
        ("string", "\"\""),
        (" int ", "0"),
        ("uint8", "0"),
        ("float64", "0"),
        ("complex128", "0"),
        ("rune", "0"),
        ("*Node", "nil"),
        ("[]int", "nil"),
        ("map[string]int", "nil"),
        ("chan int", "nil"),
        ("func(int) bool", "nil"),
        ("interface{}", "nil"),
        ("interface {}", "nil"),
        ("any", "nil"),
        ("error", "nil"),
        ("Point", "Point{}"),
        ("pkg.T", "pkg.T{}"),
        ("[3]int", "[3]int{}"),
    ];
    for (ty, expected) in cases {
        assert_eq!(go_zero_value(ty), *expected, "{ty:?}");
    }
}

fn decl() -> StructDecl {
    StructDecl {
        name: "S".into(),
        language: "go".into(),
        fields: ["a", "b", "c"]
            .into_iter()
            .map(|name| FieldDecl {
                name: name.into(),
                ty: "int".into(),
                vis: "pub".into(),
            })
            .collect(),
        generics: None,
        is_pub: true,
        decl_start: 0,
        decl_end: 0,
        line: 1,
        col: 1,
    }
}

fn site(values: &[(&str, &str)]) -> InstantiationSite {
    InstantiationSite {
        start: 0,
        end: 0,
        field_values: values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        field_order: vec![],
        prefix: String::new(),
        has_rest_pattern: false,
    }
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

#[test]
fn go_literal_values_follow_declaration_order() {
    let d = decl();
    #[allow(clippy::type_complexity)]
    let cases: &[(&[(&str, &str)], Vec<Option<String>>)] = &[
        (&[], vec![None, None, None]),
        (
            &[("__positional_0", "1"), ("__positional_1", "2")],
            vec![some("1"), some("2"), None],
        ),
        (&[("__positional_1", "x")], vec![None, some("x"), None]),
        (&[("b", "2")], vec![None, some("2"), None]),
        (&[("c", "3"), ("a", "1")], vec![some("1"), None, some("3")]),
    ];
    for (idx, (values, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &go_literal_values(&site(values), &d).unwrap(),
            expected,
            "case {idx}"
        );
    }
}

#[test]
fn go_literal_values_refusals() {
    let d = decl();
    let cases: &[(&[(&str, &str)], &str)] = &[
        (
            &[("__positional_0", "1"), ("a", "2")],
            "Go literals cannot mix keyed and positional elements",
        ),
        (
            &[
                ("__positional_0", "1"),
                ("__positional_1", "2"),
                ("__positional_2", "3"),
                ("__positional_3", "4"),
            ],
            "Go positional literal has more values than declared fields",
        ),
        (&[("z", "1")], "Go literal refers to unknown field `z`"),
    ];
    for (idx, (values, message)) in cases.iter().enumerate() {
        let err = go_literal_values(&site(values), &d).expect_err("must refuse");
        assert_eq!(err.to_string(), *message, "case {idx}");
    }
}
