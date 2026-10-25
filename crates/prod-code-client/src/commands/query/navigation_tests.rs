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
