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
    ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, ProdCodeCodec, SyncProbeResponse,
    SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

pub struct Step {
    pub stdout: String,
    pub code: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
    pub changed: bool,
}

impl Step {
    pub fn green() -> Self {
        Self {
            stdout: String::new(),
            code: Some(0),
            timed_out: false,
            error: None,
            changed: false,
        }
    }

    pub fn rust(output: &str) -> Self {
        Self {
            stdout: format!("{output}\n"),
            ..Self::green()
        }
    }
}

/// Finite, explicitly joined exec-protocol peer; Workspace supplies the existing sync fixture.
pub struct Gateway {
    pub addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Vec<ExecRequest>>,
}

impl Gateway {
    pub async fn start(steps: Vec<Step>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut steps = steps.into_iter();
            let mut requests = Vec::new();
            loop {
                let socket = tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap().0,
                };
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    match message {
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
                        WireMessage::ExecRequest(req) => {
                            let step = steps.next().unwrap_or_else(Step::green);
                            framed
                                .send(WireMessage::ExecChunk(ExecChunk {
                                    stderr: false,
                                    data: Some(step.stdout.into_bytes()),
                                }))
                                .await
                                .unwrap();
                            if step.changed {
                                framed
                                    .send(WireMessage::ExecChanges(ExecChanges {
                                        files: vec![FileDelta {
                                            relative_path: "main.py".into(),
                                            content: Some(b"value = 2\n".to_vec()),
                                            is_executable: false,
                                        }],
                                    }))
                                    .await
                                    .unwrap();
                            }
                            framed
                                .send(WireMessage::ExecExit(ExecExit {
                                    exit_code: step.code,
                                    timed_out: step.timed_out,
                                    error: step.error,
                                    duration_ms: 1,
                                    server_workspace_root: req.client_workspace_root.clone(),
                                    usage: None,
                                    platform: None,
                                }))
                                .await
                                .unwrap();
                            requests.push(req);
                            break;
                        }
                        other => panic!("unexpected fixture request {other:?}"),
                    }
                }
            }
            requests
        });
        Self { addr, stop, task }
    }

    pub async fn finish(self) -> Vec<ExecRequest> {
        self.stop.send(()).unwrap();
        self.task.await.unwrap()
    }
}
