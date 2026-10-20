/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Unit tests for `code_grep` (`GrepOptions`, `run_grep`, `find_enclosing_symbol_in_lines`) (#1119).

use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn grep_finds_literal_and_regex_matches_with_enclosing_symbols() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        r#"pub struct SessionPool {
    cap: usize,
}

impl SessionPool {
    pub fn acquire_connection(&self, url: &str) -> Result<(), String> {
        if url.is_empty() {
            return Err("connection reset by peer".to_string());
        }
        Ok(())
    }
}

#[test]
fn test_acquire_connection_error() {
    let pool = SessionPool { cap: 4 };
    assert!(pool.acquire_connection("").is_err());
}
"#,
    )
    .unwrap();

    // Also create a target/ file that must be excluded by default.
    let target_dir = root.join("target/debug");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(
        target_dir.join("ignored.rs"),
        "fn ignored() { let _ = \"connection reset by peer\"; }\n",
    )
    .unwrap();

    // 1. Literal search for error string inside a method
    let opts = GrepOptions {
        pattern: "connection reset by peer".to_string(),
        fixed_strings: true,
        context: 1,
        ..Default::default()
    };
    let report = run_grep(root, None, &opts).await.unwrap();
    assert_eq!(report.hits.len(), 1);
    let hit = &report.hits[0];
    assert_eq!(hit.path, "src/lib.rs");
    assert_eq!(hit.line, 8);
    assert_eq!(
        hit.enclosing_symbol.as_deref(),
        Some("SessionPool::acquire_connection")
    );
    assert_eq!(hit.context_before.len(), 1);
    assert_eq!(hit.context_after.len(), 1);
    let rendered = report.render();
    assert!(rendered.contains("[in SessionPool::acquire_connection]"));
    assert!(rendered.contains("src/lib.rs:8:"));

    // 2. Prefix regex search `fn test_`
    let prefix_opts = GrepOptions {
        pattern: "fn test_".to_string(),
        ..Default::default()
    };
    let prefix_report = run_grep(root, None, &prefix_opts).await.unwrap();
    assert_eq!(prefix_report.hits.len(), 1);
    assert_eq!(prefix_report.hits[0].line, 15);
    assert_eq!(
        prefix_report.hits[0].enclosing_symbol.as_deref(),
        Some("test_acquire_connection_error")
    );
}

#[tokio::test]
async fn grep_respects_include_exclude_globs_and_max_results() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    std::fs::write(
        root.join("server.go"),
        r#"package main

func (s *Gateway) ServeHTTP() {
    // TODO: wire metrics
    // TODO: wire healthcheck
}
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("server_test.go"),
        r#"package main

func TestServeHTTP() {
    // TODO: add assertions
}
"#,
    )
    .unwrap();

    let opts = GrepOptions {
        pattern: "TODO".to_string(),
        include: vec!["*.go".to_string()],
        exclude: vec!["*_test.go".to_string()],
        max_results: 1,
        ..Default::default()
    };
    let report = run_grep(root, None, &opts).await.unwrap();
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.total_matches, 2);
    assert!(report.truncated);
    assert_eq!(report.hits[0].path, "server.go");
    assert_eq!(
        report.hits[0].enclosing_symbol.as_deref(),
        Some("Gateway.ServeHTTP")
    );
}

#[test]
fn enclosing_symbol_handles_python_and_typescript() {
    let py_lines: Vec<String> = r#"class WorkerPool:
    def dispatch_task(self, payload: str) -> bool:
        if not payload:
            return False
        return True
"#
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(
        find_enclosing_symbol_in_lines(&py_lines, 4).as_deref(),
        Some("WorkerPool.dispatch_task")
    );

    let ts_lines: Vec<String> = r#"export class RpcClient {
    async sendRequest(method: string): Promise<void> {
        console.log(method);
    }
}
"#
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(
        find_enclosing_symbol_in_lines(&ts_lines, 3).as_deref(),
        Some("RpcClient::sendRequest")
    );
}

#[tokio::test]
async fn execute_tool_code_grep_and_schema_registration() {
    let tools = crate::tools::build_tools_raw();
    assert!(
        tools.iter().any(|t| t.name == "code_grep"),
        "code_grep must be registered in MCP build_tools_raw()"
    );

    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("main.rs"),
        "pub fn handle_request() {\n    let msg = \"unexpected EOF\";\n}\n",
    )
    .unwrap();

    let remote: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let res = crate::tools::execute_tool(
        remote,
        root,
        "code_grep",
        serde_json::json!({
            "pattern": "unexpected EOF",
            "fixed_strings": true,
        }),
    )
    .await
    .unwrap();
    assert!(!res.is_error);
    let crate::protocol::McpContentItem::Text { text } = &res.content[0];
    assert!(text.contains("main.rs:2:"));
    assert!(text.contains("[in handle_request]"));
}
