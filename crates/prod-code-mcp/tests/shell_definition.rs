/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_definition` by name finds a function in a shell script and returns its body, though no
//! language server answers a shell symbol (#1940).

use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

async fn gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _| match method {
        "prod-code/handshake" => json!({ "index_gated": true }),
        "workspace/symbol" => json!([]),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn a_shell_function_is_found_by_name_with_its_body() {
    let ws = Workspace::new(&[
        (
            "scripts/util.sh",
            "#!/bin/bash\n\n# Greets.\nfunction greet {\n  echo \"hello $1\"\n}\n\nother() {\n  :\n}\n",
        ),
        ("README.md", "greet is documented here\n"),
    ]);
    let gateway = gateway().await;
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_definition",
        json!({ "symbol": "greet", "body": true }),
    )
    .await
    .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains("No definition found"), "{text}");
    assert!(text.contains("scripts/util.sh:4:"), "{text}");
    assert!(text.contains("echo"), "the body is returned: {text}");
    assert!(
        !text.contains("other()"),
        "the next function is not part of it: {text}"
    );
}

#[tokio::test]
async fn two_shell_functions_of_one_name_are_listed_not_guessed() {
    let ws = Workspace::new(&[
        ("a/lib.sh", "build() {\n  echo a\n}\n"),
        ("b/lib.sh", "build() {\n  echo b\n}\n"),
    ]);
    let gateway = gateway().await;
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_definition",
        json!({ "symbol": "build" }),
    )
    .await
    .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert!(
        text.contains("a/lib.sh") && text.contains("b/lib.sh"),
        "{text}"
    );
}

#[tokio::test]
async fn a_name_no_script_defines_is_still_an_error_not_a_guess() {
    let ws = Workspace::new(&[("scripts/util.sh", "greet() {\n  :\n}\n")]);
    let gateway = gateway().await;
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_definition",
        json!({ "symbol": "missing_function" }),
    )
    .await;
    let text = match result {
        Ok(result) => serde_json::to_string(&result).unwrap(),
        Err(error) => format!("{error:#}"),
    };
    assert!(!text.contains("util.sh:"), "{text}");
}
