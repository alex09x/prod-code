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
use crate::tools::{execute_tool, resolve_symbol};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const DEPENDENCY: &str = "pub struct Dep;\nimpl Dep {\n    pub fn method(&self) -> u8 { 7 }\n    pub fn method_more(&self) {}\n}\npub trait DepTrait {\n    fn trait_method(&self);\n}\n";

fn owned(name: &str, path: &Path, line: u32, col: u32, owner: &str) -> Value {
    let mut symbol = answers::symbol(name, 12, path, line, col);
    symbol["containerName"] = json!(owner);
    symbol
}

#[tokio::test]
async fn symbols_dependency_fallback_table_preserves_workspace_precedence_and_body() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn dep() {}\npub fn method_more_file() {}\npub struct Shadow;\nimpl Shadow {\n    pub fn method(&self) -> u8 { 1 }\n}\n",
        ),
    ]);
    let local = ws.root().join("src/lib.rs");
    let dependency = ws
        .root()
        .parent()
        .unwrap()
        .join("dependency-source/src/lib.rs");
    assert!(
        !dependency.exists(),
        "dependency source is served only by the node"
    );
    let queries = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&queries);
    let local_path = local.clone();
    let dep_path = dependency.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "prod-code/handshake" => json!({"index_gated": true}),
        "prod-code/readFile" => json!(DEPENDENCY),
        "workspace/symbol" => {
            let query = params["query"].as_str().unwrap();
            if params["searchScope"] == "dependencies" {
                assert_eq!(params["prodCodeTimeoutMs"], 5000);
                seen.lock().unwrap().push(query.to_owned());
                match query {
                    "Dep::method" | "dep_crate::Dep::method" => {
                        json!([owned("method", &dep_path, 3, 12, "dep_crate::Dep")])
                    }
                    "DepTrait::trait_method" => json!([owned(
                        "trait_method",
                        &dep_path,
                        7,
                        8,
                        "dep_crate::DepTrait"
                    )]),
                    "Dep::methdo" => json!([
                        owned("method", &dep_path, 3, 12, "dep_crate::Dep"),
                        owned("method_more", &dep_path, 4, 12, "dep_crate::Dep"),
                        owned("method_other", &dep_path, 4, 12, "dep_crate::Other")
                    ]),
                    _ => json!([]),
                }
            } else if query == "Shadow::method" || query == "method" {
                json!([owned("method", &local_path, 5, 12, "fixture::Shadow")])
            } else if query.eq_ignore_ascii_case("dep") {
                // An exact workspace function with a different case hides the dependency type.
                json!([answers::symbol("dep", 12, &local_path, 1, 8)])
            } else {
                json!([answers::symbol("method_more_file", 12, &local_path, 2, 8)])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        "textDocument/definition" => json!([{
            "uri": params["textDocument"]["uri"],
            "range": {"start": params["position"], "end": params["position"]}
        }]),
        _ => Value::Null,
    })
    .await;

    for (query, path, line, col, extra_searches) in [
        ("Dep::method", &dependency, 3, 12, 1),
        ("dep_crate::Dep::method", &dependency, 3, 12, 1),
        ("Shadow::method", &local, 5, 12, 0),
        ("DepTrait::trait_method", &dependency, 7, 8, 1),
    ] {
        let before = queries.lock().unwrap().len();
        let hit = resolve_symbol(gateway.addr(), &ws.root(), query, None)
            .await
            .unwrap();
        assert_eq!((&hit.path, hit.line, hit.col), (path, line, col), "{query}");
        assert_eq!(
            queries.lock().unwrap().len() - before,
            extra_searches,
            "{query}"
        );
    }
    let missing = resolve_symbol(gateway.addr(), &ws.root(), "Dep::methdo", None)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        missing,
        "no symbol named `Dep::methdo` in the workspace index; did you mean: dep_crate::Dep::method, dep_crate::Dep::method_more"
    );
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_definition",
        json!({
            "symbol": "dep_crate::Dep::method"
        }),
    )
    .await
    .unwrap();
    assert!(!result.is_error);
    let output = serde_json::to_string(&result).unwrap();
    assert!(
        output.contains(&format!("{}:3:12", dependency.display())),
        "{output}"
    );
    assert!(
        output.contains("3 |     pub fn method(&self) -> u8 { 7 }"),
        "{output}"
    );
}

#[tokio::test]
async fn symbols_dependency_scope_rejects_malformed_exact_positions() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn unused() {}\n"),
    ]);
    let gateway = ScriptedGateway::start(|_, _| {
        json!([{
            "name": "method", "location": {"uri": "file:///dependency/src/lib.rs"}
        }])
    })
    .await;
    let error = dependency_symbol_search(gateway.addr(), &ws.root(), "Dep::method", &["Dep"], None)
        .await
        .unwrap_err();
    assert!(is_malformed_lsp_coordinate(&error), "{error:#}");
}

#[tokio::test]
async fn symbols_dependency_scope_timeout_bounds_an_unresponsive_gateway() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn unused() {}\n"),
    ]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let started = std::time::Instant::now();
    let result = dependency_symbol_search(address, &ws.root(), "Dep::method", &["Dep"], None).await;
    server.abort();
    let _ = server.await;
    assert!(result.unwrap().is_empty());
    assert!(started.elapsed() >= DEPENDENCY_SEARCH_BUDGET);
    assert!(started.elapsed() < DEPENDENCY_SEARCH_BUDGET + Duration::from_secs(2));
}
