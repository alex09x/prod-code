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
use super::workspace;
use serde_json::{Value, json};
use std::process::Command;

const IMPL: &str = "macro_rules! owner { ($ty:ty)=>{$ty} }\nstruct First;\nstruct Second;\nimpl owner! { First }\n{\n    fn run()\n    {\n        missing();\n    }\n}\n";

/// The type macro is legal Rust; only the deliberate unresolved call may fail.
fn compiler_control(label: &str, text: &str) -> Value {
    let ws = workspace(text);
    let output = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-type=lib",
            "--error-format=json",
            "src/lib.rs",
            "-o",
            "control.rlib",
        ])
        .current_dir(ws.root())
        .output()
        .expect("verified remote Rust compiler");
    let stderr = String::from_utf8(output.stderr).unwrap();
    eprintln!(
        "{label} source:\n{text}{label} rustc exit={:?}:\n{stderr}",
        output.status.code()
    );
    assert_eq!(output.status.code(), Some(1));
    let errors: Vec<Value> = stderr
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|d| {
            d["level"] == "error" && !d["message"].as_str().unwrap().starts_with("aborting")
        })
        .collect();
    assert_eq!(errors.len(), 1, "unexpected grammar/compiler errors");
    assert_eq!(errors[0]["code"]["code"], "E0425");
    let span = errors[0]["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["is_primary"] == true)
        .unwrap();
    assert_eq!(span["line_start"], 8);
    assert_eq!(span["column_start"], 9);
    assert_eq!(span["line_end"], 8);
    assert_eq!(span["column_end"], 16);
    json!({"code": errors[0]["code"]["code"], "message": errors[0]["message"],
        "range": [span["line_start"], span["column_start"], span["line_end"], span["column_end"]]})
}

#[tokio::test]
async fn impl_macro_header_changed_owner_keeps_error_blocking() {
    let after = IMPL.replace("owner! { First }", "owner! { Second }");
    let before_error = compiler_control("before", IMPL);
    let after_error = compiler_control("after", &after);
    assert_eq!(
        before_error, after_error,
        "the actual compiler error is unchanged"
    );
    assert!(check("impl target macro First -> Second", IMPL, &after, false).await);
}

#[tokio::test]
async fn impl_macro_header_unchanged_owner_preserves_baseline() {
    let mut failures = Vec::new();
    for (label, after) in [
        ("same impl snapshot", IMPL.to_owned()),
        ("impl line shift", format!("// insertion\n{IMPL}")),
        (
            "impl header trivia",
            IMPL.replace("owner! { First }", "owner /* ! { */ ! { /* } */ First }"),
        ),
        (
            "between type macro and impl body",
            IMPL.replace("First }\n{", "First } /* outer /* nested */ } */\n{"),
        ),
        (
            "impl body line shift",
            IMPL.replace(
                "        missing();",
                "        // insertion\n        missing();",
            ),
        ),
    ] {
        if !check(label, IMPL, &after, true).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "baseline lost: {failures:?}");
}

#[tokio::test]
async fn impl_macro_header_completed_items_and_siblings_end_context() {
    let mut failures = Vec::new();
    let declarations = "macro_rules! owner { ($ty:ty)=>{$ty} }\nstruct First;\nstruct Second;\nmacro_rules! items { ($($item:item)*) => { $($item)* } }\n";
    let sibling = "fn sibling()\n{\n    missing();\n}\n";
    for (label, before, after) in [
        (
            "completed impl target macro",
            "impl owner! { First }\n{ fn previous() {} }\n",
            "impl owner! { Second }\n{ fn previous() {} }\n",
        ),
        (
            "completed standalone item macro",
            "items! { struct Alpha; }\n",
            "items! { struct Beta; }\n",
        ),
        (
            "completed associated item macro",
            "impl First {\n    items! { fn previous() {} }\n",
            "impl First {\n    items! { fn renamed() {} }\n",
        ),
    ] {
        let closing = if label == "completed associated item macro" {
            "}\n"
        } else {
            ""
        };
        let before = format!("{declarations}{before}{sibling}{closing}");
        let after = format!("{declarations}{after}{sibling}{closing}");
        if !check(label, &before, &after, true).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "sibling baseline lost: {failures:?}");
}
