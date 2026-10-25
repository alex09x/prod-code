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
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::codec::Framed;

pub const FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"validate-diff-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    (
        "src/lib.rs",
        "pub mod child;\npub fn value() -> u32 { child::VALUE }\n",
    ),
    ("src/child.rs", "pub const VALUE: u32 = 1;\n"),
    ("src/unused.rs", "pub const UNUSED: u32 = 2;\n"),
];

/// LSP answers are scripted clean; compiler answers come from real compilers on a private copy,
/// using only the FileDeltas actually received across the handler/shadow wire boundary.
pub struct CompilerGateway {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<ShadowRunRequest>>>,
    pub analyzer_requests: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl CompilerGateway {
    pub async fn start() -> Self {
        Self::with_files(FILES).await
    }

    pub async fn with_files(files: &'static [(&'static str, &'static str)]) -> Self {
        Self::with_mode(files, None).await
    }

    pub async fn scripted(response: ShadowRunResponse) -> Self {
        Self::with_mode(FILES, Some(response)).await
    }

    async fn with_mode(
        files: &'static [(&'static str, &'static str)],
        response: Option<ShadowRunResponse>,
    ) -> Self {
        let analyzer_requests = Arc::new(AtomicUsize::new(0));
        let analyzed = Arc::clone(&analyzer_requests);
        let upstream = ScriptedGateway::start(move |method, _| {
            analyzed.fetch_add(1, Ordering::SeqCst);
            match method {
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => Value::Null,
            }
        })
        .await
        .addr();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (client, _) = accepted.unwrap();
                        let seen = Arc::clone(&seen);
                        let response = response.clone();
                        connections.spawn(async move {
                            let server = TcpStream::connect(upstream).await.unwrap();
                            let mut client = Framed::new(client, ProdCodeCodec::new());
                            let mut server = Framed::new(server, ProdCodeCodec::new());
                            loop {
                                tokio::select! {
                                    incoming = client.next() => match incoming {
                                        Some(Ok(WireMessage::ShadowRunRequest(req))) => {
                                            let response = response.clone().unwrap_or_else(|| compile(&req, files));
                                            seen.lock().unwrap().push(req);
                                            client.send(WireMessage::ShadowRunResponse(response))
                                                .await.unwrap();
                                        }
                                        Some(Ok(message)) => {
                                            if server.send(message).await.is_err() { return; }
                                        }
                                        _ => return,
                                    },
                                    outgoing = server.next() => match outgoing {
                                        Some(Ok(message)) => {
                                            if client.send(message).await.is_err() { return; }
                                        }
                                        _ => return,
                                    },
                                }
                            }
                        });
                    }
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            addr,
            requests,
            analyzer_requests,
            task,
        }
    }
}

impl Drop for CompilerGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn compile(request: &ShadowRunRequest, files: &[(&str, &str)]) -> ShadowRunResponse {
    assert!(
        request.command
            == [
                "cargo",
                "check",
                "--workspace",
                "--all-targets",
                "--message-format=json"
            ]
            || request.command == ["make"]
            || request.command
                == [
                    "sh",
                    "-c",
                    "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null && cmake --build build",
                ]
    );
    assert_eq!(request.hypotheses.len(), 1);
    let hypothesis = &request.hypotheses[0];
    let shadow = Workspace::new(files);
    for delta in &hypothesis.files {
        let relative = Path::new(&delta.relative_path);
        assert!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        );
        let path = shadow.root().join(relative);
        match &delta.content {
            Some(text) => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            None => match std::fs::remove_file(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => panic!("cannot delete fixture file: {error}"),
            },
        }
        match &delta.content {
            Some(bytes) => assert_eq!(
                std::fs::read(shadow.path(&delta.relative_path)).unwrap(),
                *bytes
            ),
            None => assert!(std::fs::symlink_metadata(shadow.path(&delta.relative_path)).is_err()),
        }
    }
    let output = std::process::Command::new(&request.command[0])
        .args(&request.command[1..])
        .current_dir(shadow.root().join(request.subdir.as_deref().unwrap_or("")))
        .env("CARGO_TARGET_DIR", shadow.root().join("target"))
        .env("CARGO_NET_OFFLINE", "true")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .env("GOTOOLCHAIN", "local")
        .output()
        .expect("fixture compiler available on build node");
    let combined = [output.stdout, output.stderr].concat();
    ShadowRunResponse {
        server_workspace_root: request.client_workspace_root.clone(),
        mode: "owned-test-copy".into(),
        results: vec![ShadowHypothesisResult {
            name: hypothesis.name.clone(),
            exit_code: output.status.code(),
            duration_ms: 1,
            timed_out: false,
            error: None,
            output_len: combined.len() as u64,
            output_tail: Some(combined),
        }],
        error: None,
    }
}

pub fn assert_unchanged(workspace: &Workspace) {
    for (path, text) in FILES {
        assert_eq!(workspace.read(path), *text, "checkout changed: {path}");
    }
    assert!(!workspace.path("Cargo.lock").exists());
    assert!(!workspace.path("target").exists());
    assert!(!workspace.path("src/new.rs").exists());
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(workspace.root())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "fixture checkout must remain clean"
    );
}
