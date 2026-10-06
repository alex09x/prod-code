/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::codegen::{generate_call_replacement, generate_function_code};
use super::lang::Language;
use super::tokenize::tokenize_polyglot;
use super::types::{ExtractedParam, OutputKind, PolyTokenKind};

#[test]
fn test_tokenize_polyglot_multibyte_unicode() {
    let code = "let price = 50€; let ratio = 10 / 2; let name = \"café\";";
    let tokens = tokenize_polyglot(code, Language::TypeScript);
    assert!(
        tokens
            .iter()
            .any(|t| t.text == "€" && t.kind == PolyTokenKind::Punct)
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.text == "\"café\"" && t.kind == PolyTokenKind::Str)
    );
}

#[test]
fn test_kotlin_extract_function_generation() {
    let params = vec![ExtractedParam {
        name: "ch".to_string(),
        ty: Some("Char".to_string()),
    }];
    let output = OutputKind::Expression("ch.digitToInt(16)".to_string());
    let fn_code = generate_function_code(
        "parseHexDigit",
        &params,
        &output,
        "ch.digitToInt(16)",
        Language::Kotlin,
        false,
        false,
        "",
        Some("Int"),
    );
    assert!(fn_code.contains("private fun parseHexDigit(ch: Char): Int {"));
    assert!(fn_code.contains("return ch.digitToInt(16)"));
    assert!(!fn_code.contains("return ch.digitToInt(16);"));

    let call = generate_call_replacement(
        "parseHexDigit",
        &["c".to_string()],
        &["ch".to_string()],
        &OutputKind::SingleVar {
            name: "digit".to_string(),
            is_new: true,
        },
        Language::Kotlin,
        false,
        "    ",
    );
    assert_eq!(call, "    val digit = parseHexDigit(c)");
}

#[test]
fn test_csharp_extract_function_generation() {
    let params = vec![ExtractedParam {
        name: "input".to_string(),
        ty: Some("string".to_string()),
    }];
    let output = OutputKind::Expression("input.Trim()".to_string());
    let fn_code = generate_function_code(
        "CleanInput",
        &params,
        &output,
        "input.Trim()",
        Language::Csharp,
        false,
        false,
        "    ",
        Some("string"),
    );
    assert!(fn_code.contains("private static string CleanInput(string input)"));
    assert!(fn_code.contains("return input.Trim();"));

    let call = generate_call_replacement(
        "CleanInput",
        &["raw".to_string()],
        &["input".to_string()],
        &OutputKind::SingleVar {
            name: "cleaned".to_string(),
            is_new: true,
        },
        Language::Csharp,
        false,
        "        ",
    );
    assert_eq!(call, "        var cleaned = CleanInput(raw);");
}
