/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::sync::{push_workspace_sync, workspace_identity};
use prod_code_protocol::{ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

#[tokio::test]
async fn push_workspace_sync_terminates_on_repeated_workspace_was_fresh() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(msg) = framed.next().await {
                    match msg.unwrap() {
                        WireMessage::SyncProbeRequest(req) => {
                            let _ = framed
                                .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing: Vec::new(),
                                }))
                                .await;
                        }
                        WireMessage::SyncRequest(req) => {
                            let _ = framed
                                .send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: 0,
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: true,
                                    stale_paths: Vec::new(),
                                }))
                                .await;
                        }
                        _ => {}
                    }
                }
            });
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"test-repro\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    assert!(
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "init"
        ])
        .success()
    );

    let stream = prod_code_protocol::transport::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let identity = workspace_identity(root);

    let result = push_workspace_sync(
        &mut framed,
        root,
        &identity,
        Some(std::path::Path::new("src")),
    )
    .await;
    assert!(
        result.is_err(),
        "repeated fresh workspace resets must fail cleanly"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("gateway workspace reset loop detected"),
        "error message must mention reset loop: {err}"
    );
}

#[tokio::test]
async fn concurrent_push_workspace_sync_waits_for_initial_upload_completion() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let go_mod_uploaded = Arc::new(AtomicBool::new(false));
    let server_flag = go_mod_uploaded.clone();

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let flag = server_flag.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(msg) = framed.next().await {
                    match msg.unwrap() {
                        WireMessage::SyncProbeRequest(req) => {
                            let missing: Vec<String> = if flag.load(Ordering::SeqCst) {
                                Vec::new()
                            } else {
                                req.files.into_iter().map(|f| f.relative_path).collect()
                            };
                            let _ = framed
                                .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing,
                                }))
                                .await;
                        }
                        WireMessage::SyncRequest(req) => {
                            let was_fresh = !flag.load(Ordering::SeqCst);
                            if req.files.iter().any(|f| f.relative_path == "go.mod") {
                                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                                flag.store(true, Ordering::SeqCst);
                            }
                            let _ = framed
                                .send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: req.files.len(),
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: was_fresh,
                                    stale_paths: Vec::new(),
                                }))
                                .await;
                        }
                        _ => {}
                    }
                }
            });
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("go.mod"), "module example.com/app\n\ngo 1.22\n").unwrap();
    std::fs::write(root.join("main.go"), "package main\n\nfunc main() {}\n").unwrap();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    assert!(
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "init"
        ])
        .success()
    );

    let root_a = root.to_path_buf();
    let root_b = root.to_path_buf();
    let id_a = workspace_identity(root);
    let id_b = id_a.clone();
    let flag_b = go_mod_uploaded.clone();

    let task_a = tokio::spawn(async move {
        let stream = prod_code_protocol::transport::connect(addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        push_workspace_sync(&mut framed, &root_a, &id_a, None)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    let task_b = tokio::spawn(async move {
        let stream = prod_code_protocol::transport::connect(addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        push_workspace_sync(&mut framed, &root_b, &id_b, None)
            .await
            .unwrap();
        assert!(
            flag_b.load(Ordering::SeqCst),
            "second caller must not complete pre-flight sync before go.mod is uploaded (#1098)"
        );
    });

    task_a.await.unwrap();
    task_b.await.unwrap();
}
