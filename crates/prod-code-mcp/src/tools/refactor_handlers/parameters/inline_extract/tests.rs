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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn parameter_handlers_validate_arguments_before_contacting_analyzer() {
    let ws = Workspace::new(&[]);
    let remote = "127.0.0.1:1".parse().unwrap();
    for (args, expected) in [
        (json!({}), "Missing 'path' argument"),
        (
            json!({"path":"a.rs"}),
            "Missing 'name' argument: what the new parameter is called",
        ),
        (json!({"path":"a.rs","name":"x"}), "Missing 'line' argument"),
        (
            json!({"path":"a.rs","name":"x","line":1}),
            "Missing 'character' argument",
        ),
        (
            json!({"path":"a.rs","name":"x","line":1,"character":1}),
            "Missing 'end_line' argument",
        ),
        (
            json!({"path":"a.rs","name":"x","line":1,"character":1,"end_line":1}),
            "Missing 'end_character' argument",
        ),
        (
            json!({"path":"a.ts","name":"x","verify":"compile"}),
            "`verify: compile` runs `cargo check` and is for Rust files; the analyzer's check of the result is reported without it",
        ),
    ] {
        assert_eq!(
            handle_extract_parameter(remote, &ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    for (args, expected) in [
        (json!({}), "Missing 'path' argument"),
        (
            json!({"path":"a.rs"}),
            "Missing 'line' argument for Rust inline_parameter",
        ),
        (
            json!({"path":"a.ts","verify":"compile"}),
            "verify: compile is only supported for Rust inline_parameter; no files were written",
        ),
    ] {
        assert_eq!(
            handle_inline_parameter(remote, &ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

const EXTRACT_SOURCE: &str = "fn value() -> u32 {\n    42\n}\n";
const EXTRACTED: &str = "fn value(amount: u32) -> u32 {\n    amount\n}\n";

#[tokio::test]
async fn extracting_parameter_reports_exact_edits_and_compiler_verdict() {
    for (verify, apply, diagnostic) in [
        (false, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let ws = Workspace::new(&[("sample.rs", EXTRACT_SOURCE)]);
        let gw = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => {
                json!([answers::document_symbol("value", 12, 1, 3, 4)])
            }
            "textDocument/references" => json!([]),
            "textDocument/diagnostic" if diagnostic => {
                answers::error_at(1, 1, "E0001", "invalid result")
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            "prod-code/shadow-run" => json!({"exit_code":0}),
            _ => Value::Null,
        })
        .await;
        let result = handle_extract_parameter(
            gw.addr(),
            &ws.root(),
            &json!({
                "path":"sample.rs", "line":2,"character":5,"end_line":2,"end_character":7,
                "name":"amount","type":"u32","replace_all":true,"apply":apply,
                "verify":if verify {"compile"} else {"analyzer"}
            }),
        )
        .await
        .unwrap();
        assert_eq!(result.is_error, diagnostic);
        let crate::protocol::McpContentItem::Text { text } = &result.content[0];
        assert_eq!(
            text.lines().take(5).collect::<Vec<_>>(),
            vec![
                "`value` (sample.rs)",
                "",
                "- new parameter: `amount: u32`",
                "- from the body: `42`",
                "- 1 place(s) in the body now read it, 0 call site(s) pass it"
            ]
        );
        assert_eq!(
            ws.read("sample.rs"),
            if apply { EXTRACTED } else { EXTRACT_SOURCE }
        );
        if verify && !diagnostic {
            assert_eq!(
                text.rsplit_once("\nthe compiler").unwrap().1,
                " accepts the result too: `cargo check` in a shadow of the workspace, 10 ms\n"
            );
        } else if diagnostic {
            assert_eq!(
                text.rsplit_once("\nthe compiler").unwrap().1,
                " was not asked: the analyzer already rejects the result\n"
            );
        }
    }
}

#[tokio::test]
async fn inline_rust_and_typescript_apply_exact_rewrites() {
    for (path, source, expected, args, verify) in [
        (
            "sample.rs",
            "fn value(amount: u32) -> u32 {\n    amount\n}\nfn caller() { value(42); }\n",
            "fn value() -> u32 {\n    let amount: u32 = 42;\n    amount\n}\nfn caller() { value(); }\n",
            json!({"line":1,"character":10}),
            true,
        ),
        (
            "sample.ts",
            "export function value(amount: number): number {\n    return amount;\n}\nvalue(42);\n",
            "export function value(): number {\n    const amount: number = 42;\n    return amount;\n}\nvalue();\n",
            json!({"symbol":"value","param":"amount"}),
            false,
        ),
    ] {
        let ws = Workspace::new(&[(path, source)]);
        let file = ws.path(path);
        let rust = verify;
        let gw = ScriptedGateway::start(move |method, _| match method {
            "textDocument/references" => {
                answers::locations(&file, &[(4, if rust { 15 } else { 1 })])
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            "prod-code/shadow-run" => json!({"exit_code":0}),
            _ => Value::Null,
        })
        .await;
        let mut args = args;
        args["path"] = json!(path);
        args["apply"] = json!(true);
        if verify {
            args["verify"] = json!("compile");
        }
        let result = handle_inline_parameter(gw.addr(), &ws.root(), &args)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(ws.read(path), expected);
        let crate::protocol::McpContentItem::Text { text } = &result.content[0];
        assert_eq!(
            text.lines().next(),
            Some(format!("`amount` of `value` ({path})").as_str())
        );
        assert_eq!(text.lines().nth(3), Some("- 1 call(s) lose the argument"));
    }
}
