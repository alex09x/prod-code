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
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::codec::Framed;

#[derive(Debug, Clone)]
pub struct CapturedShadowRun {
    pub request: ShadowRunRequest,
    pub exit_code: Option<i32>,
    pub compiler_output: String,
}

/// Executes real `cargo check` on a private copy of the fixture workspace using only the
/// serialized `FileDelta`s received over the shadow wire protocol.
pub struct CargoCompileGateway {
    pub addr: SocketAddr,
    pub runs: Arc<Mutex<Vec<CapturedShadowRun>>>,
    task: JoinHandle<()>,
}

impl CargoCompileGateway {
    pub async fn with_files(files: &'static [(&'static str, &'static str)]) -> Self {
        let upstream = ScriptedGateway::start(move |method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            "textDocument/references" => json!([]),
            _ => Value::Null,
        })
        .await
        .addr();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&runs);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (client, _) = accepted.unwrap();
                        let recorded = Arc::clone(&recorded);
                        connections.spawn(async move {
                            let server = TcpStream::connect(upstream).await.unwrap();
                            let mut client = Framed::new(client, ProdCodeCodec::new());
                            let mut server = Framed::new(server, ProdCodeCodec::new());
                            loop {
                                tokio::select! {
                                    incoming = client.next() => match incoming {
                                        Some(Ok(WireMessage::ShadowRunRequest(req))) => {
                                            let (response, captured) = execute_real_cargo(&req, files);
                                            recorded.lock().unwrap().push(captured);
                                            client
                                                .send(WireMessage::ShadowRunResponse(response))
                                                .await
                                                .unwrap();
                                        }
                                        Some(Ok(message)) => {
                                            if server.send(message).await.is_err() {
                                                return;
                                            }
                                        }
                                        _ => return,
                                    },
                                    outgoing = server.next() => match outgoing {
                                        Some(Ok(message)) => {
                                            if client.send(message).await.is_err() {
                                                return;
                                            }
                                        }
                                        _ => return,
                                    },
                                }
                            }
                        });
                    }
                    joined = connections.join_next(), if !connections.is_empty() => {
                        joined.unwrap().unwrap();
                    }
                }
            }
        });
        Self { addr, runs, task }
    }

    pub fn take_runs(&self) -> Vec<CapturedShadowRun> {
        std::mem::take(&mut *self.runs.lock().unwrap())
    }
}

impl Drop for CargoCompileGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn execute_real_cargo(
    request: &ShadowRunRequest,
    files: &[(&str, &str)],
) -> (ShadowRunResponse, CapturedShadowRun) {
    assert_eq!(
        request.command,
        [
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=short",
        ]
    );
    assert_eq!(request.hypotheses.len(), 1);
    let hypothesis = &request.hypotheses[0];
    assert_eq!(hypothesis.name, "proposed");

    let shadow = Workspace::new(files);
    for delta in &hypothesis.files {
        let relative = Path::new(&delta.relative_path);
        assert!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "non-normal delta path: {}",
            delta.relative_path
        );
        let target = shadow.root().join(relative);
        match &delta.content {
            Some(bytes) => {
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(&target, bytes).unwrap();
                assert_eq!(&std::fs::read(&target).unwrap(), bytes);
            }
            None => {
                if let Err(err) = std::fs::remove_file(&target) {
                    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
                }
            }
        }
    }

    let cwd = shadow.root().join(request.subdir.as_deref().unwrap_or(""));
    let output = std::process::Command::new(&request.command[0])
        .args(&request.command[1..])
        .current_dir(&cwd)
        .env("CARGO_TARGET_DIR", shadow.root().join("target"))
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("real cargo available on build node");
    let combined = [output.stdout, output.stderr].concat();
    let exit_code = output.status.code();
    let compiler_output = String::from_utf8_lossy(&combined).into_owned();
    eprintln!(
        "[shadow-cargo] subdir={:?} exit={:?} deltas={}\n{}",
        request.subdir,
        exit_code,
        hypothesis.files.len(),
        compiler_output.trim()
    );

    let response = ShadowRunResponse {
        server_workspace_root: shadow.root().to_string_lossy().into_owned(),
        mode: "overlay".into(),
        results: vec![ShadowHypothesisResult {
            name: hypothesis.name.clone(),
            exit_code,
            duration_ms: 15,
            timed_out: false,
            error: None,
            output_len: combined.len() as u64,
            output_tail: Some(combined),
        }],
        error: None,
    };
    // Opt-in full proof; ordinary acceptance never rewrites retained evidence.
    if std::env::var_os("COMPILE_PROJECTS_PROOF").is_some() {
        let mut public_request = request.clone();
        public_request.client_workspace_root = "<fixture>".into();
        public_request.base_workspace_name = None;
        public_request.client_host = None;
        let mut public_response = response.clone();
        public_response.server_workspace_root = "<fixture>".into();
        let staged: Vec<_> = hypothesis
            .files
            .iter()
            .map(|delta| {
                let bytes = std::fs::read(shadow.root().join(&delta.relative_path)).ok();
                json!({"path": delta.relative_path, "actual_staged_bytes": bytes})
            })
            .collect();
        eprintln!(
            "PROOF {}",
            json!({
                "test": std::thread::current().name(),
                "original_files": files,
                "request": public_request,
                "staged": staged,
                "command_cwd": request.subdir.as_deref().unwrap_or("."),
                "response": public_response,
                "compiler_output": compiler_output,
                "compiler_exit": exit_code,
                "duration_contract": "synthetic protocol duration 15 ms; no timing claim"
            })
        );
    }
    let captured = CapturedShadowRun {
        request: request.clone(),
        exit_code,
        compiler_output,
    };
    (response, captured)
}

pub fn assert_workspace_clean(
    workspace: &Workspace,
    files: &[(&str, &str)],
    extra_absent: &[&str],
) {
    for (path, expected) in files {
        assert_eq!(
            workspace.read(path),
            *expected,
            "workspace file modified on disk: {path}"
        );
    }
    for path in ["Cargo.lock", "target"]
        .into_iter()
        .chain(extra_absent.iter().copied())
    {
        assert!(
            !workspace.path(path).exists(),
            "unexpected artifact in checkout: {path}"
        );
    }
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(workspace.root())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "fixture checkout is dirty: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

pub fn consumer_proof(name: &str, files: &[(&str, &str)], returned: &str, workspace: &Workspace) {
    if std::env::var_os("COMPILE_PROJECTS_PROOF").is_some() {
        let pairs: Vec<_> = files
            .iter()
            .map(|(path, original)| {
                json!({
                    "path": path, "original": original, "returned_disk": workspace.read(path)
                })
            })
            .collect();
        eprintln!(
            "CONSUMER_PROOF {}",
            json!({"consumer": name, "returned": returned, "source_pairs": pairs})
        );
    }
}
