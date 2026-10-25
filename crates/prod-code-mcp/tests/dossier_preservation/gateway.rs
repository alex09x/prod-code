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
use prod_code_protocol::{
    ExecChunk, ExecExit, ExecRequest, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::codec::Framed;

pub const BUILD_ERROR: &str = "error[E0308]: mismatched types\n --> selected/src/lib.rs:7:17\n";

pub enum Reply {
    Cargo,
    Delayed(Duration, &'static str),
}

pub struct Gateway {
    pub addr: SocketAddr,
    calls: Arc<Mutex<Vec<ExecRequest>>>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Gateway {
    pub async fn start(root: &Path, replies: Vec<Reply>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let root = root.to_path_buf();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let serve = async {
                for reply in replies {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut wire = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(msg)) = wire.next().await {
                        match msg {
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
                                println!(
                                    "capture command={:?} timeout_secs={} subdir={:?}",
                                    req.command, req.timeout_secs, req.subdir
                                );
                                captured.lock().unwrap().push(req.clone());
                                let (stdout, stderr, exit_code) = match reply {
                                    Reply::Cargo => {
                                        let mut command =
                                            tokio::process::Command::new(&req.command[0]);
                                        command
                                            .args(&req.command[1..])
                                            .current_dir(
                                                req.subdir
                                                    .as_ref()
                                                    .map(|sub| root.join(sub))
                                                    .unwrap_or(root.clone()),
                                            )
                                            .envs(req.env)
                                            .env("CARGO_BUILD_JOBS", "2")
                                            .kill_on_drop(true);
                                        let out = command.output().await.unwrap();
                                        (out.stdout, out.stderr, out.status.code())
                                    }
                                    Reply::Delayed(delay, json) => {
                                        tokio::time::sleep(delay).await;
                                        if json.is_empty() {
                                            (vec![], BUILD_ERROR.as_bytes().to_vec(), Some(101))
                                        } else {
                                            (json.as_bytes().to_vec(), vec![], Some(101))
                                        }
                                    }
                                };
                                for (stderr, data) in [(false, stdout), (true, stderr)] {
                                    if !data.is_empty() {
                                        println!(
                                            "compiler stderr={stderr}: {}",
                                            String::from_utf8_lossy(&data).replace(
                                                &root.to_string_lossy().to_string(),
                                                "<fixture>"
                                            )
                                        );
                                        if wire
                                            .send(WireMessage::ExecChunk(ExecChunk {
                                                stderr,
                                                data: Some(data),
                                            }))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                                let _ = wire
                                    .send(WireMessage::ExecExit(ExecExit {
                                        exit_code,
                                        duration_ms: 0,
                                        server_workspace_root: root.to_string_lossy().into_owned(),
                                        timed_out: false,
                                        error: None,
                                        usage: None,
                                        platform: None,
                                    }))
                                    .await;
                                break;
                            }
                            other => panic!("unexpected message: {other:?}"),
                        }
                    }
                }
            };
            tokio::select! { _ = serve => {}, _ = &mut stopped => {} }
        });
        Self {
            addr,
            calls,
            stop,
            task,
        }
    }

    pub async fn finish(self) -> Vec<ExecRequest> {
        let _ = self.stop.send(());
        self.task.await.unwrap();
        Arc::try_unwrap(self.calls).unwrap().into_inner().unwrap()
    }
}

pub fn workspace() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for dir in ["selected/src", "selected/benches", "other/src"] {
        std::fs::create_dir_all(temp.path().join(dir)).unwrap();
    }
    for (path, text) in [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"selected\", \"other\"]\nresolver = \"2\"\n",
        ),
        (
            "selected/Cargo.toml",
            "[package]\nname = \"selected\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[[bench]]\nname = \"broken\"\nharness = false\n",
        ),
        (
            "other/Cargo.toml",
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "other/src/lib.rs",
            "compile_error!(\"unselected package must not be built\");\n",
        ),
        (
            "selected/src/lib.rs",
            include_str!("fixtures/test_build.txt"),
        ),
        (
            "selected/benches/broken.rs",
            include_str!("fixtures/benchmark.txt"),
        ),
    ] {
        std::fs::write(temp.path().join(path), text).unwrap();
    }
    temp
}
