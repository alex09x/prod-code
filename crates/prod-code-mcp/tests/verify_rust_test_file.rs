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
use prod_code_mcp::verify::{VerifyKind, run_verify_with};
use prod_code_protocol::{ExecExit, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage};
use tokio_util::codec::Framed;

async fn gateway() -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            let response = match msg {
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
                WireMessage::ExecRequest(req) => {
                    let command = req.command;
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(0),
                            duration_ms: 1,
                            server_workspace_root: req.client_workspace_root,
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await
                        .unwrap();
                    return command;
                }
                _ => continue,
            };
            framed.send(response).await.unwrap();
        }
        panic!("expected an exec request")
    });
    (address, task)
}

#[tokio::test]
async fn integration_file_selects_only_its_cargo_target() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("src/lib.rs"),
        "#[test] fn unrelated() { panic!(\"must not run\"); }\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("tests")).unwrap();
    let target = root.path().join("tests/parity_scroll.rs");
    std::fs::write(&target, "#[test] fn intended() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(std::path::Path::new("tests/parity_scroll.rs")),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(
        command,
        ["cargo", "test", "-p", "demo", "--test", "parity_scroll"]
    );
    let output = std::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(root.path())
        .env("CARGO_TARGET_DIR", root.path().join("target"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("intended ... ok"));
    let library = root.path().join("src/lib.rs");
    std::fs::write(&library, "#[test] fn unit() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(&library),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(command, ["cargo", "test", "-p", "demo"]);
}

#[tokio::test]
async fn explicit_test_target_preserves_filter_and_package() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname='demo'\nversion='0.1.0'\nautotests=false\n[[test]]\nname='custom'\npath='qa/suite.rs'\n").unwrap();
    std::fs::create_dir_all(root.path().join("qa")).unwrap();
    let target = root.path().join("qa/suite.rs");
    std::fs::write(&target, "#[test] fn wanted() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        Some("wanted"),
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(
        command,
        ["cargo", "test", "-p", "demo", "--test", "custom", "wanted"]
    );
}

#[tokio::test]
async fn integration_file_in_a_workspace_member_keeps_its_package() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers=['crates/app']\nresolver='2'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("crates/app/tests")).unwrap();
    std::fs::write(
        root.path().join("crates/app/Cargo.toml"),
        "[package]\nname='app'\nversion='0.1.0'\n",
    )
    .unwrap();
    let target = root.path().join("crates/app/tests/check.rs");
    std::fs::write(&target, "#[test] fn check() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(command, ["cargo", "test", "-p", "app", "--test", "check"]);
}

#[tokio::test]
async fn directory_main_is_an_automatic_integration_target() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("tests/feature")).unwrap();
    let target = root.path().join("tests/feature/main.rs");
    std::fs::write(&target, "#[test] fn check() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(
        command,
        ["cargo", "test", "-p", "demo", "--test", "feature"]
    );
}

#[tokio::test]
async fn unsupported_and_ambiguous_files_are_refused_before_connection() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        "[package]\nname='demo'\nversion='0.1.0'\nautotests=false\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("tests")).unwrap();
    let target = root.path().join("tests/orphan.rs");
    std::fs::write(&target, "#[test] fn orphan() {}\n").unwrap();
    let unreachable = "127.0.0.1:1".parse().unwrap();
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no test was run"), "{error}");
    std::fs::write(&manifest, "[package]\nname='demo'\nversion='0.1.0'\nautotests=false\n[[test]]\nname='one'\npath='tests/orphan.rs'\n[[test]]\nname='two'\npath='tests/orphan.rs'\n").unwrap();
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no test was run"), "{error}");
    std::fs::create_dir_all(root.path().join("qa")).unwrap();
    std::fs::write(root.path().join("qa/other.rs"), "#[test] fn other() {}\n").unwrap();
    std::fs::write(
        &manifest,
        "[package]\nname='demo'\nversion='0.1.0'\n[[test]]\nname='orphan'\npath='qa/other.rs'\n",
    )
    .unwrap();
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no test was run"), "{error}");
    std::fs::write(&manifest, "[package]\nname='demo'\nversion='0.1.0'\n").unwrap();
    std::fs::create_dir_all(root.path().join("tests/orphan")).unwrap();
    std::fs::write(
        root.path().join("tests/orphan/main.rs"),
        "#[test] fn other() {}\n",
    )
    .unwrap();
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no test was run"), "{error}");
    let missing = root.path().join("tests/missing.rs");
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&missing),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no test was run"), "{error}");
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    let source = root.path().join("src/lib.rs");
    std::fs::write(&source, "#[test] fn unit() {}\n").unwrap();
    let (address, server) = gateway().await;
    let report = run_verify_with(
        address,
        root.path(),
        Some(&source),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    let command = server.await.unwrap();
    assert_eq!(report.command, command);
    assert_eq!(command, ["cargo", "test", "-p", "demo"]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_test_files_are_refused_before_connection() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("tests/real")).unwrap();
    std::fs::write(
        root.path().join("tests/real/main.rs"),
        "#[test] fn selected() {}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("real", root.path().join("tests/alias")).unwrap();
    let target = root.path().join("tests/alias/main.rs");
    let unreachable = "127.0.0.1:1".parse().unwrap();
    let error = run_verify_with(
        unreachable,
        root.path(),
        Some(&target),
        VerifyKind::Test,
        None,
        30,
        &[],
        |_| {},
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("symlink") && error.contains("no test was run"),
        "{error}"
    );
}
