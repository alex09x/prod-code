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
fn forwarding_arguments_by_language() {
    type Parser = fn(&str) -> Vec<String>;
    let cases: &[(Parser, &str, &[&str])] = &[
        (extract_param_names_ts, "", &[]),
        (extract_param_names_ts, ", ,", &[]),
        (
            extract_param_names_ts,
            "public value: Map<string, number>, ...rest: string[]",
            &["value", "...rest"],
        ),
        (
            extract_param_names_ts,
            "count?: number, limit = 3, {x}: Point, bad-name: number",
            &["count", "limit"],
        ),
        (extract_param_names_py, "", &[]),
        (
            extract_param_names_py,
            "self: Store, /, value: tuple[int, int]=(1, 2), , *, flag=True, **options",
            &["value", "flag=flag", "**options"],
        ),
        (
            extract_param_names_py,
            "self, *items, flag: bool, **options, bad-name=0",
            &["*items", "flag=flag", "**options"],
        ),
        (
            extract_param_names_cpp,
            " , const Point &point, char *label = nullptr, Map<A, B> values, bad-name",
            &["point", "label", "values"],
        ),
        (extract_param_names_cpp, "", &[]),
        (extract_param_names_swift, "", &[]),
        (
            extract_param_names_swift,
            ", _ value: Int, with label: String, flag: Bool, !: Int",
            &["value", "with: label", "flag: flag"],
        ),
        (extract_param_names_go, "", &[]),
        (
            extract_param_names_go,
            ", a, b int, items ...string, bad-name int,\r\n value map[string]int",
            &["a", "b", "items", "value"],
        ),
    ];
    for (parser, input, expected) in cases {
        assert_eq!(parser(input), *expected, "{input:?}");
    }
}
