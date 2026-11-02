/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_mcp::watch::current_generation;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const CARGO_TOML: &str =
    "[package]\nname = \"live_non_git_repro\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const INITIAL_LIB: &str = "pub fn workflow_unique(value: u32) -> u32 { helper(value) + 1 }\n\
                           fn helper(value: u32) -> u32 { value * 2 }\n";

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|McpContentItem::Text { text }| text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a running gateway on PROD_CODE_LIVE_GATEWAY or 127.0.0.1:9400"]
async fn live_gateway_non_git_rust_and_python_definition_and_refresh() {
    let addr_str =
        std::env::var("PROD_CODE_LIVE_GATEWAY").unwrap_or_else(|_| "127.0.0.1:9400".to_string());
    let remote: SocketAddr = addr_str.parse().expect("valid gateway socket addr");

    let rust_tmp = tempfile::tempdir().expect("rust tempdir");
    let rust_root = std::fs::canonicalize(rust_tmp.path()).expect("canonical rust root");
    std::fs::create_dir_all(rust_root.join("src")).unwrap();
    std::fs::write(rust_root.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(rust_root.join("src/lib.rs"), INITIAL_LIB).unwrap();

    let t0 = Instant::now();
    let sym = execute_tool(
        remote,
        &rust_root,
        "code_symbols",
        serde_json::json!({ "query": "workflow_unique", "path": "src/lib.rs" }),
    )
    .await
    .unwrap();
    assert!(!sym.is_error, "{}", text_of(&sym));
    let def1 = execute_tool(
        remote,
        &rust_root,
        "code_definition",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "budget_ms": 15_000 }),
    )
    .await
    .unwrap();
    assert!(
        !def1.is_error && text_of(&def1).contains("workflow_unique"),
        "{}",
        text_of(&def1)
    );
    let rust_initial_ms = t0.elapsed().as_millis();

    let g0 = current_generation(&rust_root);
    std::fs::write(
        rust_root.join("src/lib.rs"),
        format!("{INITIAL_LIB}pub fn refreshed_live(v: u32) -> u32 {{ helper(v) + 10 }}\n"),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while current_generation(&rust_root) == g0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let t1 = Instant::now();
    let def2 = execute_tool(
        remote,
        &rust_root,
        "code_definition",
        serde_json::json!({ "symbol": "refreshed_live", "budget_ms": 15_000 }),
    )
    .await
    .unwrap();
    assert!(
        !def2.is_error && text_of(&def2).contains("refreshed_live"),
        "{}",
        text_of(&def2)
    );
    let rust_refresh_ms = t1.elapsed().as_millis();

    let py_tmp = tempfile::tempdir().expect("python tempdir");
    let py_root = std::fs::canonicalize(py_tmp.path()).expect("canonical python root");
    std::fs::write(
        py_root.join("pyproject.toml"),
        "[project]\nname = \"live-py-repro\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        py_root.join("mod.py"),
        "def answer() -> int:\n    return 42\n",
    )
    .unwrap();

    let t2 = Instant::now();
    let py_def = execute_tool(
        remote,
        &py_root,
        "code_definition",
        serde_json::json!({ "symbol": "answer", "budget_ms": 15_000 }),
    )
    .await
    .unwrap();
    assert!(
        !py_def.is_error && text_of(&py_def).contains("answer"),
        "{}",
        text_of(&py_def)
    );
    println!(
        "rust_initial_ms={rust_initial_ms} rust_refresh_ms={rust_refresh_ms} py_initial_ms={}",
        t2.elapsed().as_millis()
    );
}
