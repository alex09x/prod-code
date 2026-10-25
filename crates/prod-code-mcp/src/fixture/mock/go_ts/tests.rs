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
fn defaults_for_collections_and_opaque_types() {
    for (ty, expected) in [
        ("string[]", "[]"),
        ("Array<number>", "[]"),
        ("string", "\"\""),
        ("number", "0"),
        ("boolean", "false"),
        ("void", "undefined"),
        ("any", "null"),
        ("unknown", "null"),
        ("Date", "new Date(0)"),
        ("Record<string, number>", "{}"),
        ("Widget", "{} as Widget"),
    ] {
        assert_eq!(ts_default_for_type(ty), expected, "{ty}");
    }
    for (ty, expected) in [
        ("", ""),
        (
            "func(int, int), map[string]Pair[int, string], struct { X int; Y string }",
            "nil, nil, *new(struct { X int; Y string })",
        ),
        (
            "(error, bool, string, int, float64)",
            "nil, false, \"\", 0, 0.0",
        ),
        (
            "*Widget, []int, map[string]int, chan int, <-chan int, func(), interface {}",
            "nil, nil, nil, nil, nil, nil, nil",
        ),
        ("Widget", "*new(Widget)"),
    ] {
        assert_eq!(go_default_returns(ty), expected, "{ty}");
    }
}

#[test]
fn unnamed_go_argument_and_no_return() {
    let method = MethodSignature {
        name: "Write".into(),
        params: vec![("".into(), "string".into())],
        return_type: None,
    };
    assert_eq!(
        generate_go_mock("Store", &[method], &[]),
        "// MockStore is a mock implementation of Store for testing.\ntype MockStore struct {\n    WriteFunc func(string)\n    Calls []string\n}\n\nfunc (m *MockStore) Write(arg0 string) {\n    m.Calls = append(m.Calls, \"Write\")\n    if m.WriteFunc != nil {\n        m.WriteFunc(arg0)\n        return\n    }\n}"
    );
}
