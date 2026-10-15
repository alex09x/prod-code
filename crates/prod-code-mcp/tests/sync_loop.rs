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
    use tokio::sync::Notify;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let go_mod_uploaded = Arc::new(AtomicBool::new(false));
    let server_flag = go_mod_uploaded.clone();
    let upload_entered = Arc::new(Notify::new());
    let server_entered = upload_entered.clone();
    let release_upload = Arc::new(Notify::new());
    let server_release = release_upload.clone();

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let flag = server_flag.clone();
            let entered = server_entered.clone();
            let release = server_release.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(msg) = framed.next().await {
                    match msg.unwrap() {
                        WireMessage::SyncProbeRequest(req) => {
                            let missing: Vec<String> = if flag.load(Ordering::SeqCst) {
                                Vec::new()
                            } else {
                                vec!["go.mod".to_string()]
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
                            if req.files.iter().any(|f| f.relative_path == "go.mod") {
                                entered.notify_one();
                                release.notified().await;
                                flag.store(true, Ordering::SeqCst);
                            }
                            let _ = framed
                                .send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: req.files.len(),
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: false,
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

    // Prime the persisted watermark for this node so subsequent plans have `initial = false`,
    // while `PROBED_NODES` has not yet recorded first contact in this process (#1098).
    let node_key = addr.to_string();
    let initial_plan =
        prod_code_mcp::sync::prepare_workspace_sync_for(root, &node_key, None).unwrap();
    assert!(initial_plan.initial);
    prod_code_mcp::sync::commit_workspace_sync(root, &initial_plan);
    let warm_plan = prod_code_mcp::sync::prepare_workspace_sync_for(root, &node_key, None).unwrap();
    assert!(
        !warm_plan.initial,
        "persisted watermark must make subsequent plans non-initial"
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

    // Wait until task_a has probed and entered its repair SyncRequest upload for go.mod.
    upload_entered.notified().await;

    let mut task_b = tokio::spawn(async move {
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

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut task_b)
            .await
            .is_err(),
        "second sync must wait while the first-contact repair upload is in flight"
    );

    release_upload.notify_one();
    task_a.await.unwrap();
    task_b.await.unwrap();
}

#[tokio::test]
async fn partial_probe_upload_resyncs_when_workspace_reset_before_upload() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let probe_count = Arc::new(AtomicUsize::new(0));
    let server_probes = probe_count.clone();
    let go_mod_uploaded = Arc::new(AtomicBool::new(false));
    let server_go_mod = go_mod_uploaded.clone();

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let probes = server_probes.clone();
            let has_go_mod = server_go_mod.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(msg) = framed.next().await {
                    match msg.unwrap() {
                        WireMessage::SyncProbeRequest(req) => {
                            let round = probes.fetch_add(1, Ordering::SeqCst);
                            let missing = if round == 0 {
                                // First probe sees go.mod present and only main.go missing.
                                vec!["main.go".to_string()]
                            } else {
                                // After reset, all manifest files are reported missing.
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
                            if req.files.iter().any(|f| f.relative_path == "go.mod") {
                                has_go_mod.store(true, Ordering::SeqCst);
                            }
                            let _ = framed
                                .send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: req.files.len(),
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    // Simulate the mirror being wiped right before the first partial upload.
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

    let node_key = addr.to_string();
    let initial_plan =
        prod_code_mcp::sync::prepare_workspace_sync_for(root, &node_key, None).unwrap();
    prod_code_mcp::sync::commit_workspace_sync(root, &initial_plan);

    let stream = prod_code_protocol::transport::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let identity = workspace_identity(root);

    push_workspace_sync(&mut framed, root, &identity, None)
        .await
        .unwrap();
    assert_eq!(probe_count.load(Ordering::SeqCst), 2);
    assert!(
        go_mod_uploaded.load(Ordering::SeqCst),
        "reset during partial probe upload must trigger full resync that uploads go.mod"
    );
}
