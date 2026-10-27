/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1474: after a signature change, only references that are calls are reported as unadjusted
//! call sites. An import and the function used as a value have no argument list to update.

use prod_code_mcp::impact;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const SIBLING: &str =
    "use crate::helper;\npub fn caller() -> i32 {\n    let f = helper;\n    helper(10) + f(1)\n}\n";

#[tokio::test]
async fn imports_and_function_values_are_not_unadjusted_calls() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub mod sibling;\npub fn helper(x: i32) -> i32 {\n    x + 1\n}\n",
        ),
        ("src/sibling.rs", SIBLING),
    ]);
    let root = ws.root();
    ws.write(
        "src/lib.rs",
        "pub mod sibling;\npub fn helper(x: i32, extra: bool) -> i32 {\n    if extra { x } else { x + 1 }\n}\n",
    );
    let sib_uri = prod_code_protocol::path::file_uri(ws.path("src/sibling.rs").as_path());
    let at = |line: u32, character: u32| {
        serde_json::json!({ "uri": sib_uri, "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 6 } } })
    };
    // The import, the value, and the one real call, as rust-analyzer lists their names.
    let references = serde_json::json!([at(0, 11), at(2, 12), at(3, 4)]);
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("helper", 12, 2, 4, 8)])
        }
        "textDocument/references" => references.clone(),
        "textDocument/prepareCallHierarchy" | "callHierarchy/incomingCalls" => {
            serde_json::json!([])
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 2)
        .await
        .expect("analysis runs");
    assert_eq!(
        report.signature_warnings.len(),
        1,
        "{:?}",
        report.signature_warnings
    );
    let sites: Vec<(u32, u32)> = report.signature_warnings[0]
        .unadjusted_call_sites
        .iter()
        .map(|site| (site.line, site.col))
        .collect();
    assert_eq!(sites, [(4, 5)], "only `helper(10)` is a call to adjust");
}
