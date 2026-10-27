/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A diagnostic streamed during `check --events` names its file the way the final report does,
//! not by the node's absolute path (#1479).

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::verify::{RunEvent, VerifyKind, run_verify_with};
use prod_code_protocol::{
    ExecChunk, ExecExit, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use tokio_util::codec::Framed;

const SERVER_ROOT: &str = "/srv/node/workspaces/sample";

/// A gateway whose copy of the checkout lives at [`SERVER_ROOT`] and whose compiler prints
/// `line` once.
async fn gateway(line: String) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut wire = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(message)) = wire.next().await {
            let reply = match message {
                WireMessage::SyncProbeRequest(_) => {
                    WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: SERVER_ROOT.into(),
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    })
                }
                WireMessage::SyncRequest(_) => WireMessage::SyncResponse(SyncResponse {
                    server_workspace_root: SERVER_ROOT.into(),
                    files_updated: 0,
                    files_deleted: 0,
                    bytes_transferred: 0,
                    duration_ms: 0,
                    workspace_was_fresh: false,
                    stale_paths: vec![],
                }),
                WireMessage::ExecRequest(_) => {
                    wire.send(WireMessage::ExecChunk(ExecChunk {
                        stderr: false,
                        data: Some(format!("{line}\n").into_bytes()),
                    }))
                    .await
                    .unwrap();
                    WireMessage::ExecExit(ExecExit {
                        exit_code: Some(101),
                        duration_ms: 5,
                        server_workspace_root: SERVER_ROOT.into(),
                        timed_out: false,
                        error: None,
                        usage: None,
                        platform: None,
                    })
                }
                _ => continue,
            };
            if wire.send(reply).await.is_err() {
                break;
            }
        }
    });
    addr
}

fn cargo_error(file: &str) -> String {
    serde_json::json!({
        "reason": "compiler-message",
        "message": {
            "level": "error",
            "code": {"code": "E0308"},
            "message": "mismatched types",
            "spans": [{"file_name": file, "line_start": 2, "column_start": 3, "is_primary": true}],
            "children": []
        }
    })
    .to_string()
}

async fn check(
    root: &std::path::Path,
    path: Option<&std::path::Path>,
    line: String,
) -> (Vec<RunEvent>, Vec<String>) {
    let addr = gateway(line).await;
    let mut events = Vec::new();
    let report = run_verify_with(
        addr,
        root,
        path,
        VerifyKind::Check,
        None,
        30,
        &[],
        |event| events.push(event),
    )
    .await
    .unwrap();
    let files = report
        .diagnostics
        .iter()
        .filter_map(|d| d.file.clone())
        .collect();
    (events, files)
}

fn event_files(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Diagnostic(d) => d.file.clone(),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_streamed_diagnostic_names_its_file_like_the_final_report() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::write(
        checkout.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (events, report) = check(
        checkout.path(),
        None,
        cargo_error(&format!("{SERVER_ROOT}/src/lib.rs")),
    )
    .await;
    assert_eq!(report, ["src/lib.rs"]);
    assert_eq!(
        event_files(&events),
        report,
        "the event and the report agree"
    );
}

#[tokio::test]
async fn a_nested_project_streams_checkout_relative_paths() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join(".git")).unwrap();
    let nested = checkout.path().join("tools/inner");
    std::fs::create_dir_all(nested.join("src")).unwrap();
    std::fs::write(
        nested.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (events, report) = check(
        checkout.path(),
        Some(&nested),
        cargo_error(&format!("{SERVER_ROOT}/tools/inner/src/lib.rs")),
    )
    .await;
    assert_eq!(report, ["tools/inner/src/lib.rs"], "{events:?}");
    assert_eq!(event_files(&events), report);
}
