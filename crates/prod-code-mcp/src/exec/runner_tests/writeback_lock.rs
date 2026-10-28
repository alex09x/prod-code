/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{End, changes, delta, exit, gateway};
use crate::exec::runner::run_remote_for_tool;
use crate::tools::execution::workspace_mutation_lock;
use std::time::Duration;

#[tokio::test]
async fn command_writeback_waits_for_active_reader() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("lib.rs"), "fn old() {}\n").unwrap();
    let (address, server) = gateway(
        vec![
            changes(vec![delta("lib.rs", Some("fn new() {}\n"))]),
            exit(0),
        ],
        End::Close,
    )
    .await;
    let reader = workspace_mutation_lock(root.path()).read_owned().await;
    let path = root.path().to_path_buf();
    let run_path = path.clone();
    let mut run = tokio::spawn(async move {
        run_remote_for_tool(
            address,
            &run_path,
            None,
            vec!["test".into()],
            vec![],
            20,
            true,
            |_, _| {},
        )
        .await
    });
    server.await.unwrap();
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut run)
        .await
        .is_err();
    let before = std::fs::read_to_string(path.join("lib.rs")).unwrap();
    drop(reader);
    let outcome = run.await.unwrap().unwrap();
    assert!(waited, "writeback must wait for the reader");
    assert_eq!(before, "fn old() {}\n");
    assert_eq!(
        std::fs::read_to_string(path.join("lib.rs")).unwrap(),
        "fn new() {}\n"
    );
    assert_eq!(outcome.pulled_files, ["lib.rs"]);
}
