/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// LSP `SymbolKind` number for a rust-analyzer symbol kind name.
pub(crate) fn lsp_symbol_kind(kind: &str) -> u64 {
    match kind {
        "Module" | "CrateRoot" => 2,
        "TypeAlias" | "Impl" | "SelfType" => 5,
        "Method" => 6,
        "Field" => 8,
        "Enum" => 10,
        "Trait" => 11,
        "Function" | "Fn" | "Macro" | "ProcMacro" => 12,
        "Const" | "Constant" => 14,
        "Struct" | "Union" => 23,
        "Variant" => 22,
        "TypeParam" | "ConstParam" | "LifetimeParam" => 26,
        _ => 13,
    }
}

pub(crate) fn lsp_range(line: u32, col: u32, end_line: u32, end_col: u32) -> serde_json::Value {
    serde_json::json!({
        "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        "end": { "line": end_line.saturating_sub(1), "character": end_col.saturating_sub(1) }
    })
}

pub(crate) fn hierarchy_item_json(
    item: &prod_code_engine_rust::HierarchyItem,
) -> serde_json::Value {
    serde_json::json!({
        "name": item.name,
        "kind": lsp_symbol_kind(&item.kind),
        "uri": file_uri(&item.path),
        "range": lsp_range(item.line, item.col, item.end_line, item.end_col),
        "selectionRange": lsp_range(item.line, item.col, item.line, item.col + item.name.chars().count() as u32),
    })
}

/// Call hierarchy and implementation queries on the in-memory Rust engine, in LSP shape.
pub(crate) fn hierarchy_query(
    snapshot: &prod_code_engine_rust::RustEngineSnapshot,
    method: &str,
    path: &std::path::Path,
    line: u32,
    col: u32,
) -> anyhow::Result<serde_json::Value> {
    Ok(match method {
        "textDocument/prepareCallHierarchy" => serde_json::Value::Array(
            snapshot
                .prepare_call_hierarchy(path, line, col)?
                .iter()
                .map(hierarchy_item_json)
                .collect(),
        ),
        "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
            let incoming = method == "callHierarchy/incomingCalls";
            let edges = if incoming {
                snapshot.incoming_calls(path, line, col)?
            } else {
                snapshot.outgoing_calls(path, line, col)?
            };
            serde_json::Value::Array(
                edges
                    .iter()
                    .map(|edge| {
                        let ranges: Vec<_> = edge
                            .call_sites
                            .iter()
                            .map(|(l, c)| lsp_range(*l, *c, *l, *c))
                            .collect();
                        let mut value = serde_json::json!({
                            if incoming { "from" } else { "to" }: hierarchy_item_json(&edge.item),
                            "fromRanges": ranges,
                        });
                        if incoming {
                            value["isTest"] = serde_json::Value::Bool(edge.is_test);
                        }
                        value
                    })
                    .collect(),
            )
        }
        "textDocument/diagnostic" => {
            let items: Vec<serde_json::Value> = snapshot
                .diagnostics(path)?
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "range": lsp_range(d.line, d.col, d.end_line, d.end_col),
                        "severity": match d.severity.as_str() { "error" => 1, "warning" => 2, "weak" => 4, _ => 3 },
                        "code": d.code,
                        "source": "rust-analyzer",
                        "message": d.message,
                        "tags": if d.unused { vec![1] } else { Vec::<u32>::new() },
                    })
                })
                .collect();
            serde_json::json!({ "kind": "full", "items": items })
        }
        "textDocument/implementation" => serde_json::Value::Array(
            snapshot
                .goto_implementation(path, line, col)?
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "uri": file_uri(&t.path),
                        "range": lsp_range(t.line, t.col, t.line, t.col),
                    })
                })
                .collect(),
        ),
        other => anyhow::bail!("unsupported hierarchy method {other}"),
    })
}
