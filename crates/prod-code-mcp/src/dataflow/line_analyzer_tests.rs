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
fn coverage_def_use_language_forms() {
    for (text, definitions, uses, assignment, mutation) in [
        ("for &x in items {", vec!["x"], vec!["items"], true, false),
        (
            "for (const x of items)",
            vec!["x"],
            vec!["items"],
            true,
            false,
        ),
        (
            "for i, v := range items",
            vec!["i", "v"],
            vec!["items"],
            true,
            false,
        ),
        (
            "a, b := source",
            vec!["a", "b"],
            vec!["source"],
            true,
            false,
        ),
        (
            "let mut x: Type = input;",
            vec!["x"],
            vec!["input"],
            true,
            false,
        ),
        ("const x = input;", vec!["x"], vec!["input"], true, false),
        ("var x", vec!["x"], vec![], true, false),
        (
            "items[index] = value",
            vec!["items"],
            vec!["items", "index", "value"],
            true,
            true,
        ),
        ("target = value", vec!["target"], vec!["value"], true, false),
        ("x == y", vec![], vec!["x", "y"], false, false),
        ("x != y", vec![], vec!["x", "y"], false, false),
        ("x <= y", vec![], vec!["x", "y"], false, false),
        ("x >= y", vec![], vec!["x", "y"], false, false),
        ("", vec![], vec![], false, false),
    ] {
        assert_eq!(
            parse_line_def_use(text, text),
            (
                definitions.into_iter().map(str::to_string).collect(),
                uses.into_iter().map(str::to_string).collect(),
                assignment,
                mutation
            ),
            "{text}"
        );
    }
    for op in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="] {
        let text = format!("total {op} delta");
        assert_eq!(
            parse_line_def_use(&text, &text),
            (
                vec!["total".into()],
                vec!["total".into(), "delta".into()],
                true,
                true
            )
        );
    }
    for method in [
        "push", "insert", "append", "extend", "update", "remove", "clear", "add", "set", "put",
        "pop", "sort",
    ] {
        let text = format!("items.{method}(value)");
        // The line analyzer is lexical: the method name is listed among the uses too, unless it
        // is a keyword or builtin of some language (`append` is Go's).
        let (defs, uses, assignment, mutation) = parse_line_def_use(&text, &text);
        let variables: Vec<String> = uses.into_iter().filter(|u| u != method).collect();
        assert_eq!(
            (defs, variables, assignment, mutation),
            (
                vec!["items".into()],
                vec!["items".into(), "value".into()],
                true,
                true
            ),
            "{text}"
        );
    }
    for text in [
        "if ready",
        "if(x)",
        "else",
        "elif x:",
        "for x in xs:",
        "while(x)",
        "loop {",
        "match x",
        "switch(x)",
        "case x",
        "guard x",
    ] {
        assert!(is_control_statement(text), "{text}");
    }
    assert!(!is_control_statement("return x"));
}
