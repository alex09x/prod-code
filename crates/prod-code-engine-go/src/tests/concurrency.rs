/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::Duration;

#[cfg(unix)]
use super::fake_server::fake_engine;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_did_open_and_close_on_same_uri_preserves_strict_lsp_ordering() {
    let (_dir, engine) = fake_engine(None, Duration::from_secs(5)).await;
    let uri = "file:///workspace/test.go";

    // Spawn 10 concurrent did_open calls for the exact same URI.
    let mut tasks = Vec::new();
    for i in 0..10 {
        let eng = engine.clone();
        let content = format!("package main\nvar x = {i}\n");
        tasks.push(tokio::spawn(
            async move { eng.did_open(uri, &content).await },
        ));
    }

    for task in tasks {
        task.await.expect("join").expect("did_open success");
    }

    // Synchronize with the fake server via prodCode/seen to guarantee all preceding
    // notification frames have been read and processed into the server's seen list.
    let barrier = engine
        .send_request("prodCode/seen", serde_json::json!({}))
        .await
        .expect("fake server barrier");
    let seen_arr = barrier["result"].as_array().expect("array of seen methods");
    let methods: Vec<&str> = seen_arr
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|m| m.starts_with("textDocument/did"))
        .collect();

    assert!(!methods.is_empty(), "expected did* notifications");
    // The very first notification for a document MUST be didOpen, never didChange
    assert_eq!(
        methods[0], "textDocument/didOpen",
        "first notification must be didOpen: {methods:?}"
    );
    // Every subsequent notification for the opened document must be didChange
    for (idx, method) in methods.iter().enumerate().skip(1) {
        assert_eq!(
            *method, "textDocument/didChange",
            "notification at index {idx} must be didChange: {methods:?}"
        );
    }

    // Now test concurrent did_close and did_open
    let close_engine = engine.clone();
    let open_engine = engine.clone();
    let (res_close, res_open) = tokio::join!(
        tokio::spawn(async move { close_engine.did_close(uri).await }),
        tokio::spawn(async move {
            open_engine
                .did_open(uri, "package main\nvar z = 99\n")
                .await
        }),
    );
    res_close.expect("join").expect("did_close success");
    res_open.expect("join").expect("did_open success");

    // Barrier after concurrent close/open ensures child process has consumed both frames
    let barrier_after = engine
        .send_request("prodCode/seen", serde_json::json!({}))
        .await
        .expect("fake server barrier after close/open");
    let seen_after = barrier_after["result"]
        .as_array()
        .expect("array of seen methods after close/open");
    let all_methods: Vec<&str> = seen_after
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|m| m.starts_with("textDocument/did"))
        .collect();

    let last_method = *all_methods.last().expect("last notification");
    let final_version = engine.open_files.read().await.get(uri).copied();

    // Verify open_files state strictly matches the last serialized notification
    match last_method {
        "textDocument/didClose" => {
            assert!(
                final_version.is_none(),
                "didClose was the last notification, so document must be closed in open_files: {all_methods:?}"
            );
        }
        "textDocument/didOpen" | "textDocument/didChange" => {
            assert!(
                final_version.is_some(),
                "open/change was the last notification, so document must be open in open_files: {all_methods:?}"
            );
        }
        other => panic!("unexpected last notification: {other}"),
    }
}
