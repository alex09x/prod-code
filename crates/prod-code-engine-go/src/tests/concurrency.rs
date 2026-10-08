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
    let (dir, engine) = fake_engine(None, Duration::from_secs(5)).await;
    let seen_file = dir.path().join("seen");
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

    // Read recorded LSP notifications from the fake server
    let seen = std::fs::read_to_string(&seen_file).expect("read seen file");
    let methods: Vec<&str> = seen
        .lines()
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

    // Read the updated notifications log from the fake server
    let seen_after = std::fs::read_to_string(&seen_file).expect("read seen file");
    let all_methods: Vec<&str> = seen_after
        .lines()
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
