/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

/// The key under which `rpc_error` carries its error.
pub const RPC_ERROR: &str = "prod-code/rpc-error";

/// A scripted JSON-RPC error with the language server's code and message.
pub fn rpc_error(code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({ RPC_ERROR: { "code": code, "message": message } })
}

/// The key of an answer that the gateway sends as a JSON-RPC error.
pub const FAILURE: &str = "prod-code/failure";

/// A request that fails: the gateway answers with a JSON-RPC error carrying `message`, as
/// an analyzer does when it crashes, times out or refuses the request.
pub fn failure(message: &str) -> serde_json::Value {
    serde_json::json!({ FAILURE: { "code": -32603, "message": message } })
}

/// A clean pull-diagnostics report.
pub fn no_diagnostics() -> serde_json::Value {
    serde_json::json!({ "kind": "full", "items": [] })
}

/// A pull-diagnostics report with one error at a 1-based line and column.
pub fn error_at(line: u32, col: u32, code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "full", "items": [ {
        "severity": 1,
        "code": code,
        "message": message,
        "range": {
            "start": { "line": line - 1, "character": col - 1 },
            "end": { "line": line - 1, "character": col }
        }
    } ] })
}

/// A workspace edit that replaces a file wholesale — how the in-process Rust engine
/// answers a rename or a structural rewrite.
pub fn whole_file(path: &Path, old: &str, new_text: &str) -> serde_json::Value {
    serde_json::json!({ "documentChanges": [ {
        "textDocument": { "uri": uri(path), "version": null },
        "edits": [ {
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": old.lines().count(), "character": 0 }
            },
            "newText": new_text
        } ]
    } ] })
}

/// A workspace edit with one edit per occurrence — how gopls and the TypeScript server
/// answer a rename. Each spot is (line, column, length, replacement), 1-based.
pub fn ranged(path: &Path, spots: &[(u32, u32, usize, &str)]) -> serde_json::Value {
    let edits: Vec<serde_json::Value> = spots
        .iter()
        .map(|(line, col, len, text)| {
            serde_json::json!({
                "range": {
                    "start": { "line": line - 1, "character": col - 1 },
                    "end": { "line": line - 1, "character": col - 1 + *len as u32 }
                },
                "newText": text
            })
        })
        .collect();
    serde_json::json!({ "documentChanges": [ {
        "textDocument": { "uri": uri(path), "version": null },
        "edits": edits
    } ] })
}

/// Locations, as `textDocument/references` and `definition` return them. Each spot is
/// (line, column), 1-based.
pub fn locations(path: &Path, spots: &[(u32, u32)]) -> serde_json::Value {
    serde_json::Value::Array(
        spots
            .iter()
            .map(|(line, col)| {
                serde_json::json!({
                    "uri": uri(path),
                    "range": {
                        "start": { "line": line - 1, "character": col - 1 },
                        "end": { "line": line - 1, "character": col }
                    }
                })
            })
            .collect(),
    )
}

/// One `workspace/symbol` hit. `kind` is the LSP symbol kind (5 class, 12 function,
/// 23 struct).
pub fn symbol(name: &str, kind: u32, path: &Path, line: u32, col: u32) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "kind": kind,
        "location": {
            "uri": uri(path),
            "range": {
                "start": { "line": line - 1, "character": col - 1 },
                "end": { "line": line - 1, "character": col - 1 + name.chars().count() as u32 }
            }
        }
    })
}

/// One `textDocument/documentSymbol` node, covering lines `from`..=`to` (1-based) with its
/// name at `col` on `from`.
pub fn document_symbol(name: &str, kind: u32, from: u32, to: u32, col: u32) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "kind": kind,
        "range": {
            "start": { "line": from - 1, "character": 0 },
            "end": { "line": to - 1, "character": 1 }
        },
        "selectionRange": {
            "start": { "line": from - 1, "character": col - 1 },
            "end": { "line": from - 1, "character": col - 1 + name.chars().count() as u32 }
        }
    })
}

/// A `textDocument/documentSymbol` node with the ones nested in it, the way clangd and
/// sourcekit-lsp answer: a namespace holding a class holding its methods.
pub fn nested(symbol: serde_json::Value, children: Vec<serde_json::Value>) -> serde_json::Value {
    let mut symbol = symbol;
    symbol["children"] = serde_json::Value::Array(children);
    symbol
}

/// Locations of zero width, as sourcekit-lsp answers `textDocument/references`: a range
/// that starts and ends where the name starts. Each spot is (line, column), 1-based.
pub fn points(path: &Path, spots: &[(u32, u32)]) -> serde_json::Value {
    serde_json::Value::Array(
        spots
            .iter()
            .map(|(line, col)| {
                let at = serde_json::json!({ "line": line - 1, "character": col - 1 });
                serde_json::json!({ "uri": uri(path), "range": { "start": at, "end": at } })
            })
            .collect(),
    )
}

/// Markdown hover contents, the shape every engine answers with.
pub fn hover(markdown: &str) -> serde_json::Value {
    serde_json::json!({ "contents": { "kind": "markdown", "value": markdown } })
}

/// The `file://` URI of `path`, as the other answers spell it.
pub fn uri(path: &Path) -> String {
    prod_code_protocol::path::file_uri(path)
}
