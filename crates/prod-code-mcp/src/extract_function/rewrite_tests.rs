/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Hygiene of the names extraction generates (#1439).

use super::{fresh_name, rename_placeholder, with_calls};

/// Only the inserted calls of another file are renamed; its own `"fun_name"` text, a comment
/// and even a function of its own named `fun_name` stay as they were.
#[test]
fn another_file_keeps_its_own_placeholder_text() {
    let original = "fn fun_name() {}\n// see fun_name\nfn g(o: &O) -> u32 {\n    log(\"fun_name\");\n    o.a * o.b\n}\n";
    let from = original.find("o.a * o.b").unwrap();
    let to = from + "o.a * o.b".len();
    let edits = vec![(from, to, "fun_name(o)".to_string())];
    let text = with_calls(original, &edits, "price");
    assert_eq!(
        text,
        "fn fun_name() {}\n// see fun_name\nfn g(o: &O) -> u32 {\n    log(\"fun_name\");\n    price(o)\n}\n"
    );
}

/// In the file the helper is made in, strings and comments keep the placeholder's spelling.
#[test]
fn the_placeholder_is_renamed_only_as_a_name() {
    assert_eq!(
        rename_placeholder(
            "let s = \"fun_name\"; /* fun_name */ fun_name(x) + fun_names",
            "f"
        ),
        "let s = \"fun_name\"; /* fun_name */ f(x) + fun_names"
    );
}

/// A literal's parameter takes a name nothing in the helper already binds or uses.
#[test]
fn a_literal_parameter_is_not_shadowed_by_a_local() {
    let body = "fn fun_name() -> u32 {\n    let value = 2;\n    let result = 100 + value;\n    result\n}\n";
    assert_eq!(fresh_name(body, "value", &[]), "value_2");
    assert_eq!(
        fresh_name(body, "value", &["value_2".to_string()]),
        "value_3"
    );
    assert_eq!(fresh_name(body, "value1", &[]), "value1");
    // A string or a comment that spells the name does not take it.
    assert_eq!(
        fresh_name("fn f() { log(\"value\"); } // value", "value", &[]),
        "value"
    );
}
