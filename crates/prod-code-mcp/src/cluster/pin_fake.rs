/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Fake gateways for the node pin regressions (#1793): each records every connection and every
//! frame it receives, so a test can prove what reached it and what did not.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecExit, NodePinResponse, ProdCodeCodec, StatusResponse, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use tokio_util::codec::Framed;

use super::{Endpoint, NodePolicy, PinMode};

/// The node a current fake gateway confirms.
pub(crate) const CONFIRMED: &str = "pinned-node.example:9400";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Confirms pins, syncs, and runs commands with exit 0.
    Current,
    /// Older than node pins: cannot decode the request and closes the connection.
    Old,
}

pub(crate) struct FakeGateway {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeGateway {
    pub async fn start(kind: Kind) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                log.lock().unwrap().push("connect".into());
                tokio::spawn(serve(socket, kind, log.clone()));
            }
        });
        Self { addr, seen, task }
    }

    /// Every connection (`connect`) and frame type received, in order.
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    /// The frame types received, connections left out.
    pub fn frames(&self) -> Vec<String> {
        self.seen().into_iter().filter(|s| s != "connect").collect()
    }
}

fn kind_of(message: &WireMessage) -> String {
    serde_json::to_value(message).unwrap()["type"]
        .as_str()
        .unwrap_or("?")
        .to_string()
}

async fn serve(socket: tokio::net::TcpStream, kind: Kind, seen: Arc<Mutex<Vec<String>>>) {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(Ok(message)) = framed.next().await {
        seen.lock().unwrap().push(kind_of(&message));
        let answer = match message {
            WireMessage::NodePinRequest(_) if kind == Kind::Old => return,
            WireMessage::NodePinRequest(_) => WireMessage::NodePinResponse(NodePinResponse {
                accepted: true,
                node: Some(CONFIRMED.into()),
                reason: None,
            }),
            WireMessage::StatusRequest => WireMessage::StatusResponse(StatusResponse {
                detected_engines: vec!["rust".into()],
                platform: Some("linux x86_64".into()),
                ..Default::default()
            }),
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
            WireMessage::ExecRequest(req) => WireMessage::ExecExit(ExecExit {
                exit_code: Some(0),
                duration_ms: 1,
                server_workspace_root: req.client_workspace_root,
                timed_out: false,
                error: None,
                usage: None,
                platform: Some("linux x86_64".into()),
            }),
            _ => return,
        };
        if framed.send(answer).await.is_err() {
            return;
        }
    }
}

/// A strict pin to `addr`, as `source` gave it.
pub(crate) fn strict(addr: SocketAddr, source: &str) -> NodePolicy {
    NodePolicy {
        mode: PinMode::Strict,
        endpoints: vec![Endpoint {
            name: addr.to_string(),
            addrs: vec![addr],
        }],
        source: source.into(),
    }
}

/// A small Rust checkout, canonical, with its directory guard.
pub(crate) fn checkout(extra: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("pin-gate")
        .tempdir()
        .unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"pinned\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(dir.path(), "src/lib.rs", "pub fn pinned() {}\n");
    for (path, text) in extra {
        write(dir.path(), path, text);
    }
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

pub(crate) fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}
