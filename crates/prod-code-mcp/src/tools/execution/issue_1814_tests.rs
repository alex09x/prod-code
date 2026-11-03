/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1814: a navigation call queued behind a workspace write answers at its budget, and says
//! that it was still queued rather than blaming the analyzer.

use super::{execute_tool, workspace_mutation_lock};
use prod_code_testkit::Workspace;
use std::time::{Duration, Instant};

const CARGO: &str = "[package]\nname = \"issue_1814\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1814_navigation_held_behind_a_writer_answers_at_its_budget() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub fn target() {}\n"),
    ]);
    let root = std::fs::canonicalize(ws.root()).unwrap();
    // A write in progress (an apply, a rename) holds the checkout for as long as it runs.
    let writer = workspace_mutation_lock(&root).write_owned().await;
    // Never reached: the call must give up before it asks any node.
    let nowhere = "127.0.0.1:1".parse().unwrap();
    for tool in ["code_references", "code_definition"] {
        let args = serde_json::json!({
            "path": "src/lib.rs", "line": 1, "character": 8, "budget_ms": 300
        });
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            execute_tool(nowhere, &root, tool, args),
        )
        .await
        .unwrap_or_else(|_| panic!("{tool} waited past its 300 ms budget behind the writer"))
        .expect("a busy answer, not a failure");
        let elapsed = started.elapsed();
        let text = format!("{:?}", result.content);
        assert!(result.is_error, "{tool}: {text}");
        assert!(
            elapsed < Duration::from_millis(800),
            "{tool}: {elapsed:?} {text}"
        );
        assert!(text.contains("300 ms"), "{tool}: {text}");
        assert!(
            text.contains("queued") && text.contains("workspace write"),
            "{tool} must say it waited for a write, not for the analyzer: {text}"
        );
    }
    drop(writer);
}
