/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--symbol NAME` with an explicit dependency source that exists only on the node answers
//! from that file, promptly, instead of waiting on the workspace index (#1141).

use prod_code_mcp::tools::{execute_tool, resolve_symbol};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

fn workspace(source: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", source),
    ])
}

// Use a dependency already required by this checkout, without fetching another crate.
fn registry_source() -> String {
    let lock = include_str!("../../../Cargo.lock");
    let version = lock
        .split("[[package]]")
        .find_map(|package| {
            package.contains("name = \"tokio\"\n").then(|| {
                package
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("version = \"")
                            .and_then(|v| v.strip_suffix('"'))
                    })
                    .unwrap()
            })
        })
        .unwrap();
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"));
    std::fs::read_dir(cargo_home.join("registry/src"))
        .unwrap()
        .find_map(|entry| {
            let path = entry
                .ok()?
                .path()
                .join(format!("tokio-{version}/src/task/yield_now.rs"));
            std::fs::read_to_string(path).ok()
        })
        .expect("locked tokio source must be available after compiling this test")
}

#[tokio::test]
async fn symbol_in_remote_registry_file_uses_its_outline_before_workspace_index() {
    // With an outline from the node, and with none (the file read from the node instead).
    for outlined in [true, false] {
        let ws = workspace("pub fn unrelated() {}\n");
        let source = registry_source();
        let line = source
            .lines()
            .position(|row| row.starts_with("pub async fn yield_now("))
            .unwrap() as u32
            + 1;
        // Deliberately absent locally: the script serves this dependency from the node.
        let path = ws
            .root()
            .parent()
            .unwrap()
            .join("cargo-registry/tokio/src/task/yield_now.rs");
        assert!(!path.exists());
        let outline = answers::document_symbol("yield_now", 12, line, line + 1, 14);
        let workspace_queries = Arc::new(AtomicUsize::new(0));
        let queries = Arc::clone(&workspace_queries);
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "prod-code/handshake" => json!({ "index_gated": true }),
            "prod-code/readFile" => json!(source),
            "textDocument/documentSymbol" if outlined => json!([outline]),
            "workspace/symbol" => {
                queries.fetch_add(1, Ordering::Relaxed);
                json!([])
            }
            _ => Value::Null,
        })
        .await;
        let hit = tokio::time::timeout(
            Duration::from_secs(5),
            resolve_symbol(gateway.addr(), &ws.root(), "yield_now", Some(&path)),
        )
        .await
        .expect("file lookup must not wait for workspace indexing")
        .unwrap();
        assert_eq!(hit.path, path);
        assert_eq!((hit.line, hit.col), (line, 14));
        assert_eq!(workspace_queries.load(Ordering::Relaxed), 0);
        let result = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_definition",
            json!({
                "path": path, "symbol": "yield_now", "body": false
            }),
        )
        .await
        .unwrap();
        let output = serde_json::to_string(&result).unwrap();
        assert!(output.contains(&format!(":{line}:14")), "{output}");
    }
}

#[tokio::test]
async fn python_symbol_after_many_markdown_files_is_found_by_global_lookup() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let docs = temp.path().join("a-docs");
    std::fs::create_dir_all(&docs).expect("documentation directory");
    for index in 0..5_000 {
        std::fs::write(docs.join(format!("{index:04}.md")), "plain text\n")
            .expect("write irrelevant documentation file");
    }

    let script = temp.path().join("z-scripts/cluster_rollout.py");
    std::fs::create_dir_all(script.parent().unwrap()).expect("script directory");
    let source = "def collect_cluster_nodes():\n    return []\n";
    std::fs::write(&script, source).expect("write Python declaration");

    let source = source.to_owned();
    let script_for_gateway = script.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "prod-code/handshake" => json!({ "index_gated": true }),
        "workspace/symbol" => json!([]),
        "prod-code/readFile" => json!(source),
        "textDocument/hover" => answers::hover("def collect_cluster_nodes()"),
        "textDocument/definition" => answers::locations(&script_for_gateway, &[(1, 4)]),
        _ => Value::Null,
    })
    .await;

    let hit = tokio::time::timeout(
        Duration::from_secs(15),
        resolve_symbol(gateway.addr(), temp.path(), "collect_cluster_nodes", None),
    )
    .await
    .expect("global symbol lookup must stay within its budget")
    .expect("the Python declaration is in the workspace");
    assert_eq!(hit.path, script);
    assert_eq!((hit.line, hit.col), (1, 5));

    let definition = execute_tool(
        gateway.addr(),
        temp.path(),
        "code_definition",
        json!({ "symbol": "collect_cluster_nodes" }),
    )
    .await
    .expect("code_definition resolves an unindexed Python declaration");
    let output = serde_json::to_string(&definition).unwrap();
    assert!(output.contains("def collect_cluster_nodes()"), "{output}");
}
