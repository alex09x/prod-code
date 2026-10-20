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
    let total = symbols.len();
    let mut out = String::new();
    let mut any_ok = false;

    for (idx, sym) in symbols.iter().enumerate() {
        let mut sym_args = base_args.clone();
        if let Some(obj) = sym_args.as_object_mut() {
            obj.remove("symbols");
            obj.insert("symbol".into(), serde_json::Value::String(sym.clone()));
        }
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&format!("=== [{}/{total}] {sym} ===\n", idx + 1));

        match Box::pin(super::execution::execute_tool(
            remote,
            workspace_root,
            tool_name,
            sym_args,
        ))
        .await
        {
            Ok(res) => {
                if !res.is_error {
                    any_ok = true;
                }
                for item in &res.content {
                    let McpContentItem::Text { text } = item;
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
            }
            Err(err) => {
                out.push_str(&format!("Error: {err:#}\n"));
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
