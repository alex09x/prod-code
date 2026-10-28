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
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage};
use std::time::Duration;
use tokio_util::codec::Framed;

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

#[tokio::test]
async fn reader_cannot_observe_partial_writeback_between_frames() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lib.rs");
    std::fs::write(&path, "fn old() {}\n").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            let reply = match msg {
                WireMessage::SyncProbeRequest(req) => {
                    WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    })
                }
                WireMessage::SyncRequest(req) => WireMessage::SyncResponse(SyncResponse {
                    server_workspace_root: req.client_workspace_root,
                    files_updated: 0,
                    files_deleted: 0,
                    bytes_transferred: 0,
                    duration_ms: 0,
                    workspace_was_fresh: false,
                    stale_paths: vec![],
                }),
                WireMessage::ExecRequest(_) => {
                    framed
                        .send(changes(vec![delta("lib.rs", Some("fn first() {}\n"))]))
                        .await
                        .unwrap();
                    let _ = sent_tx.send(());
                    let _ = resume_rx.await;
                    framed
                        .send(changes(vec![delta("lib.rs", Some("fn final_() {}\n"))]))
                        .await
                        .unwrap();
                    framed.send(exit(0)).await.unwrap();
                    return;
                }
                _ => continue,
            };
            framed.send(reply).await.unwrap();
        }
    });
    let run_path = root.path().to_path_buf();
    let run = tokio::spawn(async move {
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
    sent_rx.await.unwrap();
    let first_applied = tokio::time::timeout(Duration::from_secs(3), async {
        while std::fs::read_to_string(&path).unwrap() != "fn first() {}\n" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let reader_blocked = tokio::time::timeout(
        Duration::from_millis(200),
        workspace_mutation_lock(root.path()).read_owned(),
    )
    .await
    .is_err();
    let _ = resume_tx.send(());
    let result = run.await.unwrap();
    server.await.unwrap();
    assert!(first_applied.is_ok(), "the first frame was not applied");
    assert!(reader_blocked, "a reader observed partial writeback");
    assert!(result.is_ok());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "fn final_() {}\n");
}
