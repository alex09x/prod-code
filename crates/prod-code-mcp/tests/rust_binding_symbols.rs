/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Rust local declared `let mut NAME` (or `ref`, `ref mut`) resolves by NAME, at the
//! identifier, and is never offered as `mut NAME` (#1148).

use prod_code_mcp::tools::{execute_tool, resolve_symbol};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn workspace(source: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", source),
    ])
}

#[tokio::test]
async fn symbol_rust_binding_modes_resolve_references_at_identifier() {
    for mode in ["mut ", "ref ", "ref mut "] {
        let source = format!(
            "fn demo() {{\n    let {mode}items = Vec::<u64>::new();\n    items.push(1);\n}}\n"
        );
        let ws = workspace(&source);
        let path = ws.path("src/lib.rs");
        let label = format!("{mode}items");
        let column = 9 + mode.len() as u32;
        let reference_path = path.clone();
        let workspace_queries = Arc::new(AtomicUsize::new(0));
        let queries = Arc::clone(&workspace_queries);
        let gateway = ScriptedGateway::start(move |method, params| match method {
            "prod-code/handshake" => json!({ "index_gated": true }),
            "textDocument/documentSymbol" => json!([answers::nested(
                answers::document_symbol("demo", 12, 1, 4, 4),
                vec![answers::document_symbol(&label, 13, 2, 2, 9)]
            )]),
            "workspace/symbol" => {
                queries.fetch_add(1, Ordering::Relaxed);
                json!([answers::symbol("items", 12, &reference_path, 1, 4)])
            }
            "textDocument/references" | "textDocument/definition" | "textDocument/hover" => {
                assert_eq!(
                    params["position"],
                    json!({"line": 1, "character": column - 1})
                );
                match method {
                    "textDocument/references" => {
                        answers::points(&reference_path, &[(2, column), (3, 5)])
                    }
                    "textDocument/hover" => answers::hover("items: Vec<u64>"),
                    _ => Value::Null,
                }
            }
            _ => Value::Null,
        })
        .await;
        let result = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_references",
            json!({
                "path": path, "symbol": "items", "context": 3
            }),
        )
        .await
        .unwrap();
        let output = serde_json::to_string(&result).unwrap();
        assert!(output.contains("Found 2 reference(s)"), "{mode}: {output}");
        for tool in ["code_definition", "code_hover"] {
            let result = execute_tool(
                gateway.addr(),
                &ws.root(),
                tool,
                json!({
                    "path": path, "symbol": "items", "body": false
                }),
            )
            .await
            .unwrap();
            let output = serde_json::to_string(&result).unwrap();
            assert!(
                output.contains("items") || output.contains(&format!(":2:{column}")),
                "{output}"
            );
        }
        let result = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_outline",
            json!({
                "path": path, "include_locals": true
            }),
        )
        .await
        .unwrap();
        let output = serde_json::to_string(&result).unwrap();
        assert!(output.contains("[Variable] items"), "{output}");
        assert!(!output.contains("mut items"), "{output}");
        assert_eq!(workspace_queries.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn symbol_rust_binding_modes_have_file_local_fallback_when_outline_is_empty() {
    for mode in ["mut ", "ref ", "ref mut "] {
        let ws = workspace(&format!(
            "fn demo() {{\n    let {mode}items = Vec::<u64>::new();\n    items.push(1);\n}}\n"
        ));
        let path = ws.path("src/lib.rs");
        let gateway = ScriptedGateway::start(|method, _| match method {
            "prod-code/handshake" => json!({ "index_gated": true }),
            _ => json!([]),
        })
        .await;
        let hit = resolve_symbol(gateway.addr(), &ws.root(), "items", Some(&path))
            .await
            .unwrap();
        assert_eq!(hit.name, "items");
        assert_eq!((hit.line, hit.col), (2, 9 + mode.len() as u32));
    }
}
