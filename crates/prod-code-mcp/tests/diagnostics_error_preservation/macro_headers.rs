/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::const_headers::check;

const FUNCTION: &str =
    "macro_rules! output { () => { u8 } }\nfn first() -> output! {}\n{\n    missing();\n    0\n}\n";

#[tokio::test]
async fn macro_header_changed_function_keeps_error_blocking() {
    let mut failures = Vec::new();
    for (label, before) in [
        ("exact return macro", FUNCTION.to_owned()),
        (
            "qualified return macro",
            FUNCTION.replace("-> output!", "-> types::output!"),
        ),
        (
            "nested macro token trees",
            FUNCTION.replace("-> output! {}", "-> output! { { tokens; } [()] }"),
        ),
        (
            "raw identifier and macro trivia",
            FUNCTION.replace("-> output! {}", "-> r#output /* ! { */ ! /* } */ {}"),
        ),
    ] {
        if !check(
            label,
            &before,
            &before.replace("fn first", "fn second"),
            false,
        )
        .await
        {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "authority transferred: {failures:?}");
}

#[tokio::test]
async fn macro_header_unchanged_function_ignores_actual_trivia() {
    let mut failures = Vec::new();
    for (label, after) in [
        ("same snapshot", FUNCTION.to_owned()),
        ("line shift", format!("// insertion\n{FUNCTION}")),
        (
            "header trivia",
            FUNCTION.replace("-> output! {}", "-> /* { } */ output /* ! */ ! {}"),
        ),
        (
            "between macro and body",
            FUNCTION.replace("output! {}\n{", "output! {} /* outer /* nested */ } */\n{"),
        ),
        (
            "body trivia",
            FUNCTION.replace("    missing();", "    // insertion\n    missing();"),
        ),
    ] {
        if !check(label, FUNCTION, &after, true).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "baseline lost: {failures:?}");
}

#[tokio::test]
async fn macro_header_completed_items_and_siblings_end_context() {
    let mut failures = Vec::new();
    for (label, prefix, changed, suffix) in [
        (
            "completed function with macro return",
            "fn previous() -> output! {} { 0 }\n",
            "fn renamed() -> output! {} { 0 }\n",
            "",
        ),
        (
            "completed item macro",
            "items! { struct First; }\n",
            "items! { struct Second; }\n",
            "",
        ),
        (
            "completed block sibling",
            "fn outer() {\n    if true {}\n",
            "fn outer() {\n    if false {}\n",
            "}\n",
        ),
        (
            "completed never-returning function",
            "fn previous() -> ! { loop {} }\n",
            "fn renamed() -> ! { loop {} }\n",
            "",
        ),
    ] {
        let before = format!("{prefix}{FUNCTION}{suffix}");
        let after = format!("{changed}{FUNCTION}{suffix}");
        if !check(label, &before, &after, true).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "sibling baseline lost: {failures:?}");
}
