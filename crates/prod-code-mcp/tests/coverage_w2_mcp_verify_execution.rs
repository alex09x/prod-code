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
use prod_code_mcp::verify::{RunEvent, VerifyKind, compile_go_shadow, run_verify, run_verify_with};
use prod_code_protocol::{
    ExecChunk, ExecExit, ExecRequest, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use prod_code_testkit::ScriptedGateway;
use serde_json::json;
use std::net::SocketAddr;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

async fn execution_gateway(
    chunks: Vec<(bool, &'static str)>,
    error: Option<&'static str>,
) -> (SocketAddr, JoinHandle<ExecRequest>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut wire = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(message)) = wire.next().await {
            match message {
                WireMessage::SyncProbeRequest(req) => wire
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    }))
                    .await
                    .unwrap(),
                WireMessage::SyncRequest(req) => wire
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
                    .unwrap(),
                WireMessage::ExecRequest(req) => {
                    for (stderr, text) in chunks {
                        wire.send(WireMessage::ExecChunk(ExecChunk {
                            stderr,
                            data: Some(text.as_bytes().to_vec()),
                        }))
                        .await
                        .unwrap();
                    }
                    wire.send(WireMessage::ExecExit(ExecExit {
                        exit_code: if error.is_some() { None } else { Some(1) },
                        duration_ms: 17,
                        server_workspace_root: "/srv/checkout".into(),
                        timed_out: false,
                        error: error.map(str::to_owned),
                        usage: None,
                        platform: None,
                    }))
                    .await
                    .unwrap();
                    return req;
                }
                other => panic!("unexpected message: {other:?}"),
            }
        }
        panic!("no execution request");
    });
    (addr, task)
}

#[tokio::test]
async fn wave2_execute_streams_complete_lines_and_strips_failure_paths() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (addr, server) = execution_gateway(vec![
        (false, "test sample::good ... "),
        (true, "error[E0308]: mismatched types\n --> /srv/checkout/src/lib.rs:2:3\n"),
        (false, "ok\ntest sample::bad ... FAILED\n\nfailures:\n\n---- sample::bad stdout ----\n/srv/checkout/src/lib.rs: mismatch\n\nfailures:\n    sample::bad\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n"),
    ], None).await;
    let mut events = Vec::new();
    let env = vec![("RUST_BACKTRACE".into(), "1".into())];
    let report = run_verify_with(
        addr,
        temp.path(),
        None,
        VerifyKind::Test,
        Some("sample::"),
        11,
        &env,
        |event| events.push(event),
    )
    .await
    .unwrap();
    let req = server.await.unwrap();
    assert_eq!(req.command, ["cargo", "test", "--workspace", "sample::"]);
    assert_eq!(
        req.env,
        [
            ("CARGO_TERM_COLOR".into(), "never".into()),
            ("NO_COLOR".into(), "1".into()),
            ("RUST_BACKTRACE".into(), "1".into())
        ]
    );
    assert_eq!(req.timeout_secs, 11);
    assert!(!req.pull_changes);
    assert_eq!(
        events,
        [
            RunEvent::Test {
                name: "sample::good".into(),
                ok: true
            },
            RunEvent::Test {
                name: "sample::bad".into(),
                ok: false
            }
        ]
    );
    assert_eq!((report.tests_passed, report.tests_failed), (1, 1));
    assert_eq!(
        report.diagnostics[0].render(),
        "error: [E0308] mismatched types (src/lib.rs:2:3)"
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name, "sample::bad");
    assert_eq!(report.failures[0].output.trim(), "src/lib.rs: mismatch");
    assert_eq!(
        (report.exit_code, report.duration_ms, report.timed_out),
        (Some(1), 17, false)
    );
}

#[tokio::test]
async fn wave2_execute_nested_paths_and_start_error() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let nested = temp.path().join("web");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("package.json"), "{}").unwrap();
    let (addr, server) = execution_gateway(vec![(false,
        "/srv/checkout/web/src/a.ts(1,2): error TS2304: first\nweb/src/b.ts(2,3): error TS2304: second\nsrc/c.ts(3,4): error TS2304: third\n/external/d.ts(4,5): error TS2304: fourth\n")], None).await;
    let report = run_verify(
        addr,
        temp.path(),
        Some(&nested),
        VerifyKind::Check,
        None,
        10,
    )
    .await
    .unwrap();
    let req = server.await.unwrap();
    assert_eq!(req.subdir.as_deref(), Some("web"));
    assert_eq!(
        req.command,
        [
            "npx",
            "--no-install",
            "tsc",
            "--noEmit",
            "--pretty",
            "false"
        ]
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|d| d.file.as_deref().unwrap())
            .collect::<Vec<_>>(),
        [
            "web/src/a.ts",
            "web/src/b.ts",
            "web/src/c.ts",
            "/external/d.ts"
        ]
    );
    let (addr, server) = execution_gateway(vec![], Some("binary missing")).await;
    let error = run_verify(
        addr,
        temp.path(),
        Some(&nested),
        VerifyKind::Check,
        None,
        10,
    )
    .await
    .unwrap_err();
    server.await.unwrap();
    assert_eq!(
        error.to_string(),
        "remote check failed to start: binary missing"
    );
}

#[tokio::test]
async fn wave2_execute_missing_manifest_refuses_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let error = run_verify(
        "127.0.0.1:1".parse().unwrap(),
        temp.path(),
        None,
        VerifyKind::Check,
        None,
        1,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "no project manifest (Cargo.toml, go.mod, package.json, pyproject.toml, CMakeLists.txt, Package.swift) at {}",
            temp.path().display()
        )
    );
}

#[tokio::test]
async fn wave2_go_shadow_rejects_invalid_paths_and_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("sample.go");
    std::fs::write(&source, "package sample\n").unwrap();
    let unused = "127.0.0.1:1".parse().unwrap();
    let error = compile_go_shadow(unused, &root, &source, &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "cannot find a Go module containing {}; compiler verification refused",
            source.display()
        )
    );
    std::fs::write(root.join("go.mod"), "module sample\ngo 1.20\n").unwrap();
    let error = compile_go_shadow(unused, &root, &root, &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "{} is not a regular file inside the checkout; Go verification refused",
            root.display()
        )
    );
    let other = tempfile::tempdir().unwrap();
    let outside = other.path().join("other.go");
    std::fs::write(&outside, "package other\n").unwrap();
    let outside = outside.canonicalize().unwrap();
    let error = compile_go_shadow(unused, &root, &outside, &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "{} is not a regular file inside the checkout; Go verification refused",
            outside.display()
        )
    );
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("go.mod"), "module nested\ngo 1.20\n").unwrap();
    std::fs::write(nested.join("nested.go"), "package nested\n").unwrap();
    let error = compile_go_shadow(
        unused,
        &root,
        &nested.join("nested.go"),
        &[(source.clone(), "package sample\n".into())],
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "{} is outside the selected Go module {}; verification refused",
            source.display(),
            nested.display()
        )
    );
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "prod-code/shadow-run" {
            json!({"exit_code": 0})
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    let error = compile_go_shadow(
        gateway.addr(),
        &root,
        std::path::Path::new("sample.go"),
        &[(
            std::path::PathBuf::from("sample.go"),
            "package sample\n".into(),
        )],
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the remote gateway returned an unrecognized shadow mode \"mock\""
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), "package sample\n");
}
