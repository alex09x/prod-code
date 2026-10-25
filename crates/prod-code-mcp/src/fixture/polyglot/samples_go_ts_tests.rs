/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Exact sample values for Go, TypeScript and JavaScript types.

use super::samples::sample_value_for_type;
use crate::parameter_object::Language;
use crate::parameter_object::Language::{Go, JavaScript, TypeScript};

type Case = (
    Language,
    &'static str,
    Option<&'static str>,
    bool,
    &'static str,
);

fn check(cases: &[Case]) {
    for (lang, ty, field, randomized, want) in cases {
        let got = sample_value_for_type(*lang, ty, *field, *randomized);
        assert_eq!(
            &got, want,
            "{lang:?} type {ty:?} field {field:?} randomized={randomized}"
        );
    }
}

#[test]
fn go_default_values() {
    check(&[
        (Go, "bool", None, false, "false"),
        (Go, "string", None, false, "\"\""),
        (Go, "  int  ", None, false, "0"),
        (Go, "byte", None, false, "0"),
        (Go, "uint64", None, false, "0"),
        (Go, "float64", None, false, "0.0"),
        (Go, "time.Time", None, false, "time.Time{}"),
        (Go, "time.Duration", None, false, "0"),
        (Go, "error", None, false, "nil"),
        (Go, "*Config", None, false, "nil"),
        (Go, "[]string", None, false, "nil"),
        (Go, "map[string]int", None, false, "nil"),
        (Go, "Config", None, false, "Config{}"),
    ]);
}

#[test]
fn go_randomized_values() {
    check(&[
        (Go, "bool", None, true, "true"),
        (Go, "string", Some("UserID"), true, "\"id_9823\""),
        (Go, "string", Some("Email"), true, "\"user@example.com\""),
        (Go, "string", Some("Name"), true, "\"test_sample_name\""),
        (
            Go,
            "string",
            Some("Url"),
            true,
            "\"https://example.com/api\"",
        ),
        (
            Go,
            "string",
            Some("Uri"),
            true,
            "\"https://example.com/api\"",
        ),
        (Go, "string", Some("Token"), true, "\"tok_sec_7a8b9c\""),
        (Go, "string", Some("Host"), true, "\"sample_Host\""),
        (Go, "string", None, true, "\"sample_val\""),
        (Go, "int", Some("Port"), true, "8080"),
        (Go, "int64", Some("Age"), true, "30"),
        (Go, "uint", Some("ItemCount"), true, "100"),
        (Go, "int32", Some("Total"), true, "100"),
        (Go, "int", Some("Size"), true, "42"),
        (Go, "uintptr", None, true, "42"),
        (Go, "byte", None, true, "42"),
        (Go, "rune", None, true, "42"),
        (Go, "float64", Some("Price"), true, "99.95"),
        (Go, "float32", Some("Amount"), true, "99.95"),
        (Go, "float64", Some("Ratio"), true, "3.14"),
        (
            Go,
            "time.Time",
            None,
            true,
            "time.Date(2026, 1, 1, 12, 0, 0, 0, time.UTC)",
        ),
        (Go, "time.Duration", None, true, "5 * time.Second"),
        (Go, "error", None, true, "nil"),
        (
            Go,
            "[]string",
            None,
            true,
            "[]string{\"sample_1\", \"sample_2\"}",
        ),
        (Go, "[]byte", None, true, "[]byte(\"sample_bytes\")"),
        (Go, "[]int", None, true, "[]int{1, 2, 3}"),
        (Go, "[]bool", None, true, "[]bool{true}"),
        (Go, "[]float64", None, true, "[]float64{3.14}"),
        (Go, "[]*Item", None, true, "[]*Item{&Item{}}"),
        (Go, "*Config", None, true, "&Config{}"),
        (Go, "map[string]int", None, true, "map[string]int{}"),
        (Go, "Config", None, true, "Config{}"),
    ]);
}

#[test]
fn typescript_default_values() {
    check(&[
        (TypeScript, "boolean", None, false, "false"),
        (TypeScript, "number", None, false, "0"),
        (TypeScript, "string", None, false, "\"\""),
        (TypeScript, "void", None, false, "undefined"),
        (TypeScript, "any", None, false, "null"),
        (TypeScript, "unknown", None, false, "null"),
        (TypeScript, "Date", None, false, "new Date(0)"),
        (TypeScript, "string[]", None, false, "[]"),
        (TypeScript, "Array<number>", None, false, "[]"),
        (TypeScript, "Record<string, number>", None, false, "{}"),
        (TypeScript, "Profile", None, false, "{} as Profile"),
    ]);
}

#[test]
fn typescript_randomized_values() {
    check(&[
        (TypeScript, "boolean", None, true, "true"),
        (TypeScript, "string", Some("userId"), true, "\"id_9823\""),
        (
            TypeScript,
            "string",
            Some("email"),
            true,
            "\"user@example.com\"",
        ),
        (
            TypeScript,
            "string",
            Some("fullName"),
            true,
            "\"test_sample_name\"",
        ),
        (
            TypeScript,
            "string",
            Some("avatarUrl"),
            true,
            "\"https://example.com/api\"",
        ),
        (TypeScript, "string", Some("Host"), true, "\"sample_Host\""),
        (TypeScript, "string", None, true, "\"sample_val\""),
        (TypeScript, "number", Some("port"), true, "8080"),
        (TypeScript, "number", Some("age"), true, "30"),
        (TypeScript, "number", Some("price"), true, "99.95"),
        (TypeScript, "number", None, true, "42"),
        (
            TypeScript,
            "Date",
            None,
            true,
            "\"2026-01-01T00:00:00.000Z\"",
        ),
        (
            TypeScript,
            "string[]",
            None,
            true,
            "[\"sample_1\", \"sample_2\"]",
        ),
        (TypeScript, "number[]", None, true, "[10, 20, 30]"),
        (TypeScript, "boolean[]", None, true, "[]"),
        (TypeScript, "Array<string>", None, true, "[]"),
        (
            TypeScript,
            "Record<string, number>",
            None,
            true,
            "{\"key\": \"sample_val\"}",
        ),
        (TypeScript, "void", None, true, "undefined"),
        (TypeScript, "any", None, true, "null"),
        (TypeScript, "unknown", None, true, "null"),
        (TypeScript, "Profile", None, true, "{} as Profile"),
        (JavaScript, "number", Some("port"), true, "8080"),
        (JavaScript, "Profile", None, false, "{} as Profile"),
    ]);
}
