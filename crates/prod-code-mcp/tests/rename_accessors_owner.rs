/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_rename` with `accessors` renames only the accessors of the field's own struct, found by
//! the analyzer's definition of each `impl`'s self type, and reads a struct whose `where` stands
//! on a line of its own (#1383). Driven end to end against a scripted gateway.

use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

const CONN: &str = "pub struct Conn {\n    timeout: u64,\n}\n\nimpl Conn {\n    pub fn timeout(&self) -> u64 {\n        self.timeout\n    }\n}\n";

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// A 0-based single-line location in `path`.
fn location(path: &Path, line: u64, character: u64, len: u64) -> serde_json::Value {
    serde_json::json!({ "uri": uri(path), "range": {
        "start": { "line": line, "character": character },
        "end": { "line": line, "character": character + len } } })
}

fn request(params: &serde_json::Value) -> (String, u64) {
    let file = params
        .pointer("/textDocument/uri")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let line = params
        .pointer("/position/line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    (file, line)
}

/// `a::Conn` and `b::Conn` both have a `timeout` field and getter. Renaming `a`'s field renames
/// `a`'s getter only; `b`'s, whose container is also named `Conn`, stays byte for byte.
#[tokio::test]
async fn only_the_fields_own_struct_has_its_accessors_renamed() {
    let ws = Workspace::empty();
    ws.write("src/lib.rs", "pub mod a;\npub mod b;\n");
    let a = ws.write("src/a.rs", CONN);
    let b = ws.write("src/b.rs", CONN);
    ws.commit();
    let renamed_field = CONN
        .replace("    timeout: u64", "    deadline: u64")
        .replace("self.timeout", "self.deadline");
    let renamed_getter = CONN.replace("fn timeout(", "fn deadline(");
    let (pa, pb) = (a.clone(), b.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (file, line) = request(params);
        let path = if file == uri(&pb) { &pb } else { &pa };
        match method {
            // `impl Conn` (line 5) resolves to its own file's struct; anything else is the field.
            "textDocument/definition" if line == 4 => {
                serde_json::json!([location(path, 0, 11, 4)])
            }
            "textDocument/definition" => serde_json::json!([location(&pa, 1, 4, 7)]),
            "workspace/symbol" => match params.get("query").and_then(|q| q.as_str()) {
                Some("timeout") => serde_json::json!([
                    { "name": "timeout", "kind": 6, "containerName": "Conn", "location": location(&pa, 5, 11, 7) },
                    { "name": "timeout", "kind": 6, "containerName": "Conn", "location": location(&pb, 5, 11, 7) },
                ]),
                _ => serde_json::json!([]),
            },
            "textDocument/rename" => match (path == &pb, line) {
                (false, 1) => answers::whole_file(&pa, CONN, &renamed_field),
                (false, 5) => answers::whole_file(&pa, CONN, &renamed_getter),
                (true, 5) => answers::whole_file(&pb, CONN, &renamed_getter),
                _ => serde_json::Value::Null,
            },
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let args = serde_json::json!({
        "path": "src/a.rs", "line": 2, "character": 5, "new_name": "deadline", "accessors": true
    });
    let done = prod_code_mcp::tools::execute_tool(remote, &ws.root(), "code_rename", args)
        .await
        .expect("the rename runs");
    assert!(!done.is_error, "{done:?}");
    assert_eq!(ws.read("src/a.rs"), CONN.replace("timeout", "deadline"));
    assert_eq!(
        ws.read("src/b.rs"),
        CONN,
        "another module's struct of the same name"
    );
}

const WIDE: &str = "pub struct Conn<T>\nwhere\n    T: Clone,\n{\n    timeout: T,\n}\n";

/// A field of a struct whose `where` stands on a line of its own is a field of that struct.
#[tokio::test]
async fn a_struct_with_a_multiline_where_clause_owns_its_field() {
    let ws = Workspace::empty();
    let lib = ws.write("src/lib.rs", WIDE);
    ws.commit();
    let renamed = WIDE.replace("timeout", "deadline");
    let (l, answer) = (lib.clone(), renamed.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (_, line) = request(params);
        match method {
            "textDocument/definition" => serde_json::json!([location(&l, 4, 4, 7)]),
            "workspace/symbol" => serde_json::json!([]),
            "textDocument/rename" if line == 4 => answers::whole_file(&l, WIDE, &answer),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let args = serde_json::json!({
        "path": "src/lib.rs", "line": 5, "character": 5, "new_name": "deadline", "accessors": true
    });
    let done = prod_code_mcp::tools::execute_tool(remote, &ws.root(), "code_rename", args)
        .await
        .expect("the rename runs");
    assert!(!done.is_error, "{done:?}");
    assert_eq!(ws.read("src/lib.rs"), renamed);
}
