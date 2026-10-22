/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::protocol::{McpContentItem, McpToolCallResult};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// Extracts requested symbol names from `symbols: [...]` (and/or `symbol`) in order (#1123).
pub(crate) fn extract_batch_symbols(args: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push_unique = |raw: &str| {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !out.iter().any(|s| s == trimmed) {
            out.push(trimmed.to_string());
        }
    };

    if let Some(s) = args.get("symbol").and_then(|v| v.as_str()) {
        push_unique(s);
    } else if let Some(arr) = args.get("symbol").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push_unique(s);
            }
        }
    }

    if let Some(arr) = args.get("symbols").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push_unique(s);
            }
        }
    }
    out
}

/// Runs a read-only navigation tool across multiple symbols in a single batch call (#1123),
/// sharing the pooled gateway session and workspace sync across all symbols and reporting
/// per-symbol errors inline without failing the batch unless every symbol fails.
pub(crate) async fn execute_batch_symbols(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    base_args: serde_json::Value,
    symbols: &[String],
) -> Result<McpToolCallResult> {
    run_symbol_batch(base_args, symbols, |args| {
        Box::pin(super::execution::execute_tool(
            remote,
            workspace_root,
            tool_name,
            args,
        ))
    })
    .await
}

async fn run_symbol_batch<F, Fut>(
    base_args: serde_json::Value,
    symbols: &[String],
    mut execute: F,
) -> Result<McpToolCallResult>
where
    F: FnMut(serde_json::Value) -> Fut,
    Fut: std::future::Future<Output = Result<McpToolCallResult>>,
{
    let total = symbols.len();
    let shared_budget = if let Some(ms) = base_args.get("budget_ms").and_then(|v| v.as_u64()) {
        std::time::Duration::from_millis(ms.max(1))
    } else if let Some(secs) = base_args
        .get("budget_secs")
        .or_else(|| base_args.get("timeout_secs"))
        .and_then(|v| v.as_u64())
    {
        std::time::Duration::from_secs(secs.clamp(1, 90))
    } else {
        crate::call_tree::DEFAULT_CALL_TREE_BUDGET
    };
    let deadline = std::time::Instant::now() + shared_budget;
    let mut out = String::new();
    let mut any_ok = false;

    for (idx, sym) in symbols.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // Reserve a share for every remaining symbol. Unused time from a fast lookup is
        // available to later ones, but one slow lookup cannot spend their shares (#1168).
        let share = remaining / u32::try_from(total - idx).unwrap_or(u32::MAX);
        let mut sym_args = base_args.clone();
        if let Some(obj) = sym_args.as_object_mut() {
            obj.remove("symbols");
            obj.insert("symbol".into(), serde_json::Value::String(sym.clone()));
            obj.insert(
                "budget_ms".into(),
                serde_json::json!((share.as_millis() as u64).max(1)),
            );
        }
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&format!("=== [{}/{total}] {sym} ===\n", idx + 1));

        match tokio::time::timeout(share, execute(sym_args)).await {
            Ok(Ok(res)) => {
                if !res.is_error {
                    any_ok = true;
                }
                for item in &res.content {
                    let McpContentItem::Text { text } = item;
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
            }
            Ok(Err(err)) => {
                out.push_str(&format!("Error: {err:#}\n"));
            }
            Err(_) => {
                out.push_str(&format!(
                    "[Timed out on symbol {}/{total} (`{sym}`) after its {:.1}s share of the {:.1}s batch budget; retry this symbol alone or provide its file and position to narrow the lookup. Run `prod-code cluster` if the gateway remains busy.]\n",
                    idx + 1,
                    share.as_secs_f64(),
                    shared_budget.as_secs_f64()
                ));
            }
        }
    }

    let trimmed = out.trim_end().to_string();
    if any_ok {
        Ok(McpToolCallResult::text(trimmed))
    } else {
        Ok(McpToolCallResult::error(trimmed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn symbol_timeout_regression_preserves_later_results() {
        let symbols = vec!["Slow".into(), "Fast".into(), "Last".into()];
        let mut attempted = Vec::new();
        let result = run_symbol_batch(
            serde_json::json!({ "symbols": symbols, "budget_ms": 120 }),
            &symbols,
            |args| {
                let symbol = args["symbol"].as_str().unwrap().to_owned();
                attempted.push(symbol.clone());
                assert!(args.get("symbols").is_none());
                if symbol == "Slow" {
                    assert!(args["budget_ms"].as_u64().unwrap() <= 40);
                }
                async move {
                    if symbol == "Slow" {
                        std::future::pending::<()>().await;
                    }
                    Ok(McpToolCallResult::text(format!("definition of {symbol}")))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempted, symbols, "every symbol must get an attempt");
        assert!(!result.is_error, "later successes must be retained");
        let McpContentItem::Text { text } = &result.content[0];
        assert!(text.contains("Timed out on symbol 1/3 (`Slow`)"), "{text}");
        assert!(text.contains("definition of Fast"), "{text}");
        assert!(text.contains("definition of Last"), "{text}");
        assert!(
            text.contains("retry"),
            "timeout must say what to do: {text}"
        );
    }

    #[tokio::test]
    async fn symbol_timeout_regression_reports_every_failure() {
        let symbols = vec!["Slow".into(), "AlsoSlow".into(), "Last".into()];
        let result = run_symbol_batch(serde_json::json!({ "budget_ms": 30 }), &symbols, |_| {
            std::future::pending::<Result<McpToolCallResult>>()
        })
        .await
        .unwrap();
        assert!(result.is_error);
        let McpContentItem::Text { text } = &result.content[0];
        for (idx, symbol) in symbols.iter().enumerate() {
            assert!(
                text.contains(&format!("Timed out on symbol {}/3 (`{symbol}`)", idx + 1)),
                "{text}"
            );
        }
    }

    #[test]
    fn extract_batch_symbols_deduplicates_and_preserves_order() {
        let args = serde_json::json!({
            "symbol": "Alpha::run",
            "symbols": ["Beta::run", "Alpha::run", "  Gamma::run  ", ""]
        });
        assert_eq!(
            extract_batch_symbols(&args),
            vec!["Alpha::run", "Beta::run", "Gamma::run"]
        );
    }

    #[tokio::test]
    async fn multi_candidate_shared_budget_returns_partial_result_when_expired() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Package.swift"), "// swift-tools-version: 5.9\n").unwrap();
        std::fs::create_dir_all(root.join("Sources")).unwrap();
        std::fs::write(
            root.join("Sources/A.swift"),
            "struct Alpha {\n    func handleEvent() {}\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Sources/B.swift"),
            "struct Beta {\n    func handleEvent() {}\n}\n",
        )
        .unwrap();

        // Bind a TCP listener that accepts and holds the connection open so the first
        // candidate's LSP query exhausts the shared 30ms budget.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                sockets.push(stream);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        let res = crate::tools::execute_tool(
            addr,
            root,
            "code_references",
            serde_json::json!({
                "symbol": "handleEvent",
                "budget_ms": 30
            }),
        )
        .await
        .unwrap();

        let McpContentItem::Text { text } = &res.content[0];
        assert!(
            text.contains("`handleEvent` matched 2 candidates:"),
            "expected multi-candidate header, got: {text}"
        );
        assert!(
            text.contains("=== [1/2]"),
            "expected candidate 1 section, got: {text}"
        );
        assert!(
            text.contains("Partial result: shared server time budget")
                || text.contains("returning partial multi-candidate result"),
            "expected shared-budget partial result notice, got: {text}"
        );
        assert!(
            !text.contains("=== [2/2]"),
            "candidate 2 should be skipped once shared budget expired, got: {text}"
        );
    }
}
