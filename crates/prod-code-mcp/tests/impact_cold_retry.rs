/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1473: impact asks a freshly started server again only when it answered "no callers"
//! successfully; a request that failed is not a cold index, and is not repeated.

use prod_code_mcp::impact;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const GO_MOD: &str = "module example.com/shop\n\ngo 1.22\n";
const SHOP: &str = "package shop\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n";

async fn analyze_with(incoming: serde_json::Value) -> (impact::ImpactReport, usize) {
    let ws = Workspace::new(&[("go.mod", GO_MOD), ("shop.go", SHOP)]);
    ws.write("shop.go", &SHOP.replace("a + b", "b + a"));
    let root = ws.root();
    let uri = prod_code_protocol::path::file_uri(ws.path("shop.go").as_path());
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("Add", 12, 3, 5, 6)])
        }
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{ "name": "Add", "uri": uri, "_id": "Add" }])
        }
        "callHierarchy/incomingCalls" => {
            counted.fetch_add(1, Ordering::SeqCst);
            incoming.clone()
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");
    (report, asked.load(Ordering::SeqCst))
}

#[tokio::test]
async fn a_failed_hierarchy_request_is_not_retried_as_a_cold_index() {
    let started = std::time::Instant::now();
    let (report, asked) = analyze_with(answers::failure("the server crashed")).await;
    assert_eq!(asked, 1, "a failed request was asked again");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2400),
        "the cold waits ran: {:?}",
        started.elapsed()
    );
    assert!(
        !report.incomplete.is_empty(),
        "the failure is kept as a reason: {report:?}"
    );

    // A successful empty answer is still asked again, as a server that has not read the
    // project yet gives one.
    let (_, asked) = analyze_with(serde_json::json!([])).await;
    assert!(asked > 1, "an empty answer was believed at once ({asked})");
}
