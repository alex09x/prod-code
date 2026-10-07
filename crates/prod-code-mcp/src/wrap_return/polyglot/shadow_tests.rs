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
fn test_is_locally_shadowed_by_param() {
    let content = "function run(retry: () => void) {\n    retry();\n}\n";
    let call_at = content.find("retry();").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));

    let content_no_shadow = "function run() {\n    retry();\n}\n";
    let call_at = content_no_shadow.find("retry();").unwrap();
    assert!(!is_locally_shadowed(
        content_no_shadow,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_is_locally_shadowed_by_local_var() {
    let content = "function run() {\n    const retry = () => {};\n    retry();\n}\n";
    let call_at = content.find("retry();").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_destructured_const_binding_shadows() {
    let content = "async function run() {\n    const { retry } = local;\n    retry();\n}\n";
    let call_at = content.rfind("retry();").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_multiline_destructured_const_binding_shadows() {
    let content =
        "async function run() {\n    const {\n        retry,\n    } = local;\n    retry();\n}\n";
    let call_at = content.rfind("retry();").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_destructured_renamed_binding_shadows() {
    let content = "function run() {\n    const { orig: retry } = local;\n    retry();\n}\n";
    let call_at = content.rfind("retry();").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_does_not_shadow_from_earlier_closed_block() {
    let content = "function run() {\n    { const retry = () => {}; retry(); }\n    retry();\n}\n";
    let call_at = content.rfind("retry();").unwrap();
    assert!(!is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::TypeScript
    ));
}

#[test]
fn test_is_python_shadowed() {
    let content = "def run(retry):\n    retry()\n";
    let call_at = content.find("retry()").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::Python
    ));

    let content_no_shadow = "def run():\n    retry()\n";
    let call_at = content_no_shadow.find("retry()").unwrap();
    assert!(!is_locally_shadowed(
        content_no_shadow,
        call_at,
        "retry",
        Language::Python
    ));
}

#[test]
fn test_is_python_shadowed_multiline() {
    let content = "def run(\n    retry,\n):\n    retry()\n";
    let call_at = content.find("retry()").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::Python
    ));
}

#[test]
fn test_python_ignores_nested_function_assignments() {
    let content = "def outer():\n    def inner():\n        retry = 1\n    retry()\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(!is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::Python
    ));
}

#[test]
fn test_js_retains_var_in_closed_block() {
    let content =
        "function run() {\n    if (cond) {\n        var retry = local;\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}

#[test]
fn test_js_strips_let_in_closed_block() {
    let content =
        "function run() {\n    if (cond) {\n        let retry = local;\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(!is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}

#[test]
fn test_js_retains_var_without_initializer_in_closed_block() {
    let content = "function run() {\n    if (cond) {\n        var retry;\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}

#[test]
fn test_js_retains_var_in_nested_closed_block() {
    let content = "function run() {\n    if (cond) {\n        while (active) {\n            var retry = 1;\n        }\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}

#[test]
fn test_js_does_not_hoist_var_from_nested_function() {
    let content = "function run() {\n    function helper() {\n        var retry = local;\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(!is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}

#[test]
fn test_js_does_not_hoist_var_from_nested_function_in_block() {
    let content = "function run() {\n    if (cond) {\n        function helper() {\n            var retry = local;\n        }\n    }\n    retry();\n}\n";
    let call_at = content.rfind("retry()").unwrap();
    assert!(!is_locally_shadowed(
        content,
        call_at,
        "retry",
        Language::JavaScript
    ));
}
