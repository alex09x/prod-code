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

mod conditional;
mod constructors;
mod hierarchy;

async fn gateway(reject: bool) -> ScriptedGateway {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => {
            // Each validation checks the baseline before the proposed edit.
            let proposal = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2 == 1;
            if reject && proposal {
                answers::error_at(1, 1, "E1", "invalid edit")
            } else {
                answers::no_diagnostics()
            }
        }
        _ => Value::Null,
    })
    .await
}

fn assert_response(result: McpToolCallResult, expected: String, reject: bool) {
    assert_eq!(result.is_error, reject);
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"content": [{"type": "text", "text": expected}], "isError": reject})
    );
}

#[tokio::test]
async fn required_arguments_and_wrong_types_have_exact_errors() {
    let remote = "127.0.0.1:1".parse().unwrap();
    let root = Path::new(".");
    for tool in 0..6 {
        let required = match tool {
            0 | 1 => vec![
                ("path", "Missing 'path' argument"),
                ("type_name", "Missing 'type_name' argument"),
            ],
            2 | 3 => vec![
                ("path", "Missing 'path' argument"),
                ("class_name", "Missing 'class_name' (or `symbol`) argument"),
            ],
            4 => vec![
                ("path", "Missing 'path' argument"),
                ("sub_type", "Missing 'sub_type' (or `symbol`) argument"),
            ],
            _ => vec![
                ("path", "Missing 'path' argument"),
                ("base_name", "Missing 'base_name' argument"),
                ("method_name", "Missing 'method_name' argument"),
            ],
        };
        for (index, (key, message)) in required.iter().enumerate() {
            for invalid in [None, Some(Value::Null), Some(json!(42)), Some(json!([]))] {
                let mut args = json!({});
                for (previous, _) in &required[..index] {
                    args[*previous] = json!("example");
                }
                if let Some(value) = invalid {
                    args[*key] = value;
                }
                let error = match tool {
                    0 => handle_replace_constructor_with_factory(remote, root, &args).await,
                    1 => handle_replace_constructor_with_builder(remote, root, &args).await,
                    2 => handle_pull_up(remote, root, &args).await,
                    3 => handle_push_down(remote, root, &args).await,
                    4 => handle_replace_inheritance_with_delegation(remote, root, &args).await,
                    _ => handle_replace_conditional_with_polymorphism(remote, root, &args).await,
                }
                .unwrap_err();
                assert_eq!(error.to_string(), *message, "tool {tool}, {args}");
            }
        }
    }
}
