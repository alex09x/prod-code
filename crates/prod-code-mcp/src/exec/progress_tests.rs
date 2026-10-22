/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::runner::run_remote;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ExecExit, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage};
use std::time::Duration;
use tokio_util::codec::Framed;

#[tokio::test]
async fn exec_reports_probe_wait_and_silent_execution_then_returns_exit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            match msg {
                WireMessage::SyncProbeRequest(req) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    framed
                        .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: req.client_workspace_root,
                            seeded: false,
                            files_deleted: 0,
                            missing: vec![],
                        }))
                        .await
                        .unwrap();
                }
                WireMessage::SyncRequest(req) => {
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: req.client_workspace_root,
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                            stale_paths: vec![],
                        }))
                        .await
                        .unwrap();
                }
                WireMessage::ExecRequest(_) => {
                    tokio::time::sleep(Duration::from_secs(11)).await;
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(7),
                            duration_ms: 11000,
                            server_workspace_root: String::new(),
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await
                        .unwrap();
                    break;
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
    });
    let root = tempfile::tempdir().unwrap();
    let mut output = String::new();
    let result = run_remote(
        address,
        root.path(),
        None,
        vec!["sleep".into(), "11".into()],
        vec![],
        20,
        false,
        |stderr, bytes| {
            assert!(stderr);
            output.push_str(&String::from_utf8_lossy(bytes));
        },
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(result.exit.exit_code, Some(7));
    assert!(
        output.contains("sync probe response"),
        "missing pre-flight phase: {output}"
    );
    assert!(
        output.contains("waiting for command output or exit"),
        "silent execution: {output}"
    );
}

#[tokio::test]
async fn exec_preflight_identifies_local_sync_lock_wait() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let lock = crate::sync::cache::workspace_sync_lock(root.path(), &address.to_string());
    let mut held = Some(lock.lock().await);
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        assert!(matches!(
            framed.next().await,
            Some(Ok(WireMessage::SyncProbeRequest(_)))
        ));
        framed
            .send(WireMessage::Disconnect {
                reason: "test complete".into(),
            })
            .await
            .unwrap();
    });
    let identity = crate::sync::workspace_identity(root.path());
    let mut output = String::new();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        super::progress::prepare(address, root.path(), &identity, &mut |_, bytes| {
            let text = String::from_utf8_lossy(bytes);
            if text.contains("waiting for local workspace sync lock") {
                held.take();
            }
            output.push_str(&text);
        }),
    )
    .await
    .expect("lock wait was reported and released");
    server.await.unwrap();
    assert!(result.is_err());
    assert!(held.is_none(), "missing lock progress: {output}");
}
