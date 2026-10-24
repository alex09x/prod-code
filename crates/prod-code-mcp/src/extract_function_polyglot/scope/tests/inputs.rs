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
fn extracts_only_external_inputs_in_first_use_order() {
    let cases = [
        (Language::Python, "", "", vec![]),
        (
            Language::Python,
            "print(α + α + obj.field + missing)",
            "α = 1; obj = item; field = 2",
            vec![("α", None), ("obj", None)],
        ),
        (
            Language::Rust,
            "let local = source; local + source + ptr->field",
            "source: i32, ptr: Node, field: i32",
            vec![("source", Some("usize")), ("ptr", Some("usize"))],
        ),
        (
            Language::Go,
            "local := source; local + source",
            "source := 2",
            vec![("source", Some("int"))],
        ),
        (
            Language::Python,
            "local = source; other = local + source",
            "source = 1; local = 2; other = 3",
            vec![("source", None)],
        ),
        (
            Language::Python,
            "value += source",
            "value = 1; source = 2",
            vec![("value", None), ("source", None)],
        ),
        (
            Language::Python,
            "let 1; const return; =; 1 := source",
            "source = 2",
            vec![("source", None)],
        ),
        (
            Language::TypeScript,
            "var local = source; const another = local; source + another",
            "source: number",
            vec![("source", Some("number"))],
        ),
        (
            Language::Kotlin,
            "val local = source; local",
            "val source: Int = 2",
            vec![("source", Some("Int"))],
        ),
        (
            Language::Python,
            "'source' + 1 # source",
            "source = 1",
            vec![],
        ),
    ];
    for (lang, selection, before, expected) in cases {
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(name, ty)| ExtractedParam {
                name: name.into(),
                ty: ty.map(str::to_string),
            })
            .collect();
        assert_eq!(
            extract_input_variables(selection, before, lang),
            expected,
            "{selection:?}"
        );
    }
}
