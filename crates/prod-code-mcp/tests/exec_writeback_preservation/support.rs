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
use prod_code_mcp::exec::{RemoteOutcome, run_polyglot_remote, run_remote};
use prod_code_protocol::{
    ExecChanges, ExecExit, FileDelta, ProdCodeCodec, RemoteExecCommand, RemoteExecFormat,
    RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use std::{net::SocketAddr, path::Path};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_util::codec::Framed;

pub fn other_os() -> String {
    if cfg!(target_os = "macos") {
        "linux x86_64"
    } else {
        "macos aarch64"
    }
    .into()
}

pub fn delta(path: &str, content: Option<&str>) -> FileDelta {
    FileDelta {
        relative_path: path.into(),
        content: content.map(|s| s.as_bytes().to_vec()),
        is_executable: false,
    }
}

pub struct Gateway {
    pub address: SocketAddr,
    pub arrived: oneshot::Receiver<()>,
    pub release: oneshot::Sender<()>,
    pub task: JoinHandle<()>,
}

/// The request/response barrier orders local edits after preflight and before writeback.
/// Tests join the server; there are no timer races or detached fixture processes.
pub async fn gateway(files: Vec<FileDelta>, platform: String) -> Gateway {
    gateway_batches(vec![files], platform).await
}

pub async fn gateway_batches(batches: Vec<Vec<FileDelta>>, platform: String) -> Gateway {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrive, arrived) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let polyglot = loop {
            match framed.next().await.unwrap().unwrap() {
                WireMessage::SyncProbeRequest(req) => framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    }))
                    .await
                    .unwrap(),
                WireMessage::SyncRequest(req) => framed
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
                WireMessage::ExecRequest(_) => break false,
                WireMessage::RemoteExecRequest(_) => break true,
                other => panic!("unexpected request: {other:?}"),
            }
        };
        arrive.send(()).unwrap();
        released.await.unwrap();
        for files in batches {
            framed
                .send(WireMessage::ExecChanges(ExecChanges { files }))
                .await
                .unwrap();
        }
        let exit = if polyglot {
            WireMessage::RemoteExecResult(RemoteExecResult {
                exit_code: Some(0),
                duration_ms: 1,
                server_workspace_root: "/fixture".into(),
                timed_out: false,
                error: None,
                usage: None,
                platform: Some(platform),
                diagnostics: vec![],
                tests_passed: 0,
                tests_failed: 0,
                tests_skipped: 0,
                test_failures: vec![],
                benches: vec![],
            })
        } else {
            WireMessage::ExecExit(ExecExit {
                exit_code: Some(0),
                duration_ms: 1,
                server_workspace_root: "/fixture".into(),
                timed_out: false,
                error: None,
                usage: None,
                platform: Some(platform),
            })
        };
        framed.send(exit).await.unwrap();
    });
    Gateway {
        address,
        arrived,
        release,
        task,
    }
}

pub async fn run(
    address: SocketAddr,
    root: &Path,
    polyglot: bool,
    command: &[&str],
) -> RemoteOutcome {
    if !polyglot {
        return run_remote(
            address,
            root,
            None,
            command.iter().map(|s| (*s).into()).collect(),
            vec![],
            20,
            true,
            |_, _| {},
        )
        .await
        .unwrap();
    }
    let outcome = run_polyglot_remote(
        address,
        root,
        RemoteExecRequest {
            client_workspace_root: root.to_string_lossy().into(),
            base_workspace_name: None,
            language: RemoteExecLanguage::Rust,
            command: RemoteExecCommand::Check,
            args: vec![],
            env: vec![],
            format: RemoteExecFormat::Json,
            timeout_secs: 20,
            pull_changes: true,
            subdir: None,
            client_agent: None,
            client_host: None,
        },
        |_| {},
    )
    .await
    .unwrap();
    RemoteOutcome {
        exit: ExecExit {
            exit_code: outcome.result.exit_code,
            duration_ms: 1,
            server_workspace_root: "/fixture".into(),
            timed_out: false,
            error: None,
            usage: None,
            platform: outcome.result.platform,
        },
        pulled_files: outcome.pulled_files,
        relaid_files: outcome.relaid_files,
        kept_files: outcome.kept_files,
        platform_files: outcome.platform_files,
    }
}
