/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::{
    protocol::{McpContentItem, McpToolCallResult},
    sync::clear_sync_cache,
    tools::execute_tool,
};
use prod_code_testkit::ScriptedGateway;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use url::Url;

fn make_workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    clear_sync_cache(&root);
    for (rel, content) in files {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, content).unwrap();
    }
    (temp, root)
}

fn parse_report(result: &McpToolCallResult) -> Value {
    assert!(!result.is_error, "unexpected tool error: {result:?}");
    let McpContentItem::Text { text } = &result.content[0];
    serde_json::from_str(text).expect("valid workflow JSON report")
}

fn file_uri(path: &Path) -> String {
    Url::from_file_path(path).expect("file URI").to_string()
}

fn ws_sym(
    name: &str,
    kind: u64,
    uri: &str,
    line0: u32,
    col0: u32,
    container: Option<&str>,
) -> Value {
    let end = col0 + name.len() as u32;
    let mut sym = json!({
        "name": name,
        "kind": kind,
        "location": {
            "uri": uri,
            "range": {"start": {"line": line0, "character": col0}, "end": {"line": line0, "character": end}}
        }
    });
    if let Some(c) = container {
        sym["containerName"] = json!(c);
    }
    sym
}

fn doc_sym(name: &str, kind: u64, line0: u32, col0: u32, end_line0: u32, children: Value) -> Value {
    let end = col0 + name.len() as u32;
    json!({
        "name": name,
        "kind": kind,
        "range": {"start": {"line": line0, "character": 0}, "end": {"line": end_line0, "character": 1}},
        "selectionRange": {"start": {"line": line0, "character": col0}, "end": {"line": line0, "character": end}},
        "children": children
    })
}

#[tokio::test]
async fn workflow_symbols_adapter_scopes_path_hints_and_ranks_exact_matches_first() {
    let (_temp, root) = make_workspace(&[
        (
            "Cargo.toml",
            "[package]\nname = \"rank_probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        (
            "src/other.rs",
            "pub fn f_o_o_1() {}\npub fn f_o_o_2() {}\npub fn foo_other() {}\n",
        ),
        (
            "src/sub/target.rs",
            "pub fn foo_helper() {}\npub fn foo() {}\n",
        ),
    ]);
    let other_uri = file_uri(&root.join("src/other.rs"));
    let target_uri = file_uri(&root.join("src/sub/target.rs"));
    let doc_uri = target_uri.clone();

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
            let all = vec![
                ws_sym("f_o_o_1", 12, &other_uri, 0, 7, None),
                ws_sym("f_o_o_2", 12, &other_uri, 1, 7, None),
                ws_sym("foo_other", 12, &other_uri, 2, 7, None),
                ws_sym("foo_helper", 12, &target_uri, 0, 7, None),
                ws_sym("foo", 12, &target_uri, 1, 7, None),
            ];
            json!(all.into_iter().take(limit).collect::<Vec<_>>())
        }
        "textDocument/documentSymbol" => {
            if params.pointer("/textDocument/uri").and_then(Value::as_str) == Some(&doc_uri) {
                json!([
                    doc_sym("foo_helper", 12, 0, 7, 0, json!([])),
                    doc_sym("foo", 12, 1, 7, 1, json!([]))
                ])
            } else {
                json!([])
            }
        }
        _ => json!(null),
    })
    .await;

    let scoped = execute_tool(
        gw.addr(),
        &root,
        "code_workflow",
        json!({"steps": [{"id": "s", "tool": "symbols", "args": {"query": "foo", "path": "src/sub", "limit": 2}}]}),
    )
    .await
    .unwrap();
    let report = parse_report(&scoped);
    assert_eq!(report["status"], "complete", "{report}");
    assert_eq!(report["steps"][0]["value"]["truncated"], false, "{report}");
    let items = report["steps"][0]["value"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{report}");
    assert_eq!(
        (items[0]["path"].as_str(), items[0]["name"].as_str()),
        (Some("src/sub/target.rs"), Some("foo"))
    );
    assert_eq!(
        (items[1]["path"].as_str(), items[1]["name"].as_str()),
        (Some("src/sub/target.rs"), Some("foo_helper"))
    );

    let unscoped = execute_tool(
        gw.addr(),
        &root,
        "code_workflow",
        json!({"steps": [{"id": "s", "tool": "symbols", "args": {"query": "foo", "limit": 2}}]}),
    )
    .await
    .unwrap();
    let report = parse_report(&unscoped);
    assert_eq!(report["steps"][0]["value"]["truncated"], true, "{report}");
    let items = report["steps"][0]["value"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{report}");
    assert_eq!(items[0]["name"], "foo");
    assert!(
        items
            .iter()
            .all(|item| !item["name"].as_str().unwrap().starts_with("f_o_o")),
        "{report}"
    );
    clear_sync_cache(&root);
}

#[tokio::test]
async fn workflow_symbols_adapter_falls_back_to_type_members_for_qualified_queries() {
    let (_temp, root) = make_workspace(&[
        (
            "Cargo.toml",
            "[package]\nname = \"member_probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("src/alias.rs", "pub type Repo = Store;\n"),
        (
            "src/other.rs",
            "pub struct Other;\nimpl Other {\n    pub fn load() {}\n}\n",
        ),
        (
            "src/lib.rs",
            "pub struct Store {\n    pub count: usize,\n}\nimpl Store {\n    pub fn load(&self) -> usize {\n        self.count\n    }\n}\n",
        ),
    ]);
    let lib_uri = file_uri(&root.join("src/lib.rs"));
    let other_uri = file_uri(&root.join("src/other.rs"));
    let doc_lib_uri = lib_uri.clone();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => match params.get("query").and_then(Value::as_str) {
            Some("Store") => json!([ws_sym("Store", 23, &lib_uri, 0, 11, None)]),
            Some("Repo::load") => json!([ws_sym("load", 12, &other_uri, 2, 11, Some("Other"))]),
            _ => json!([]),
        },
        "textDocument/documentSymbol" => {
            if params.pointer("/textDocument/uri").and_then(Value::as_str) == Some(&doc_lib_uri) {
                json!([
                    doc_sym("Store", 23, 0, 11, 2, json!([])),
                    doc_sym(
                        "impl Store",
                        23,
                        3,
                        5,
                        7,
                        json!([doc_sym("load", 6, 4, 11, 6, json!([]))])
                    )
                ])
            } else {
                json!([])
            }
        }
        _ => json!(null),
    })
    .await;

    let result = execute_tool(
        gw.addr(),
        &root,
        "code_workflow",
        json!({"steps": [{"id": "s", "tool": "symbols", "args": {"query": "Repo::load", "limit": 3}}]}),
    )
    .await
    .unwrap();
    let report = parse_report(&result);
    assert_eq!(report["status"], "complete", "{report}");
    let items = report["steps"][0]["value"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{report}");
    assert_eq!(
        (
            items[0]["path"].as_str(),
            items[0]["name"].as_str(),
            items[0]["line"].as_u64()
        ),
        (Some("src/lib.rs"), Some("load"), Some(5))
    );
    clear_sync_cache(&root);
}
