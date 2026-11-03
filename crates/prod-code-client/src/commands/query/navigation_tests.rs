/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn issue_1793_cli_cross_checkout_outline_reaches_its_pinned_node() {
    let target = crate::commands::common::pin_tests::OldPinnedCheckout::new().await;
    let options = prod_code_mcp::tools::OutlineOptions {
        max_depth: 4,
        include_locals: false,
        hint: String::new(),
        kinds: None,
        exported_only: false,
        max_bytes: None,
        max_items: None,
    };
    let _ = run_symbols(target.other_node(), &target.file(), &options).await;
    assert_eq!(
        target.pin_requests(),
        1,
        "even a lexical fallback must not first try an unrelated node"
    );
}

#[tokio::test]
async fn references_deadline_returns_actionable_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(socket);
    });
    let error = run_refs(
        address,
        serde_json::json!({"symbol":"Sample","timeout_millis":10}),
    )
    .await
    .unwrap_err();
    task.abort();
    assert_eq!(
        error.to_string(),
        "code_references timed out after 0s; pass a file path hint or check gateway connectivity"
    );
}
