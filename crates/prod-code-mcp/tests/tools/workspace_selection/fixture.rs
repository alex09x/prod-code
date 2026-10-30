/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::path::{Path, PathBuf};

const PRIMARY_LIB: &str = "\
pub trait PrimarySuper {}
pub trait TargetTrait: PrimarySuper {}
pub struct PrimaryImpl;
impl TargetTrait for PrimaryImpl {}
pub fn target_fn() -> i32 {
    helper_primary()
}
pub fn helper_primary() -> i32 {
    10
}
pub fn caller_primary() -> i32 {
    let _from_primary = target_fn();
    _from_primary
}
";

const SIBLING_LIB: &str = "\
pub trait SiblingSuper {}
pub trait TargetTrait: SiblingSuper {}
pub struct SiblingImpl;
impl TargetTrait for SiblingImpl {}
pub fn target_fn() -> i32 {
    helper_sibling()
}
pub fn helper_sibling() -> i32 {
    20
}
pub fn caller_sibling() -> i32 {
    let _from_sibling = target_fn();
    _from_sibling
}
pub fn sibling_only_fn() -> &'static str {
    \"sibling-worktree-canary\"
}
";

pub(super) struct DivergentWorktrees {
    pub(super) ws: Workspace,
    pub(super) primary: PathBuf,
    pub(super) sibling: PathBuf,
}

impl DivergentWorktrees {
    pub(super) fn create() -> Self {
        let ws = rust_workspace(PRIMARY_LIB);
        commit(&ws);
        let primary = std::fs::canonicalize(ws.root()).expect("canonicalize primary");
        let sibling_raw = alternate_worktree(&ws);
        std::fs::write(sibling_raw.join("src/lib.rs"), SIBLING_LIB).expect("write sibling lib.rs");
        let sibling = std::fs::canonicalize(&sibling_raw).expect("canonicalize sibling");
        Self {
            ws,
            primary,
            sibling,
        }
    }

    pub(super) fn cleanup(&self) {
        git(
            &self.ws.root(),
            &[
                "worktree",
                "remove",
                "--force",
                self.sibling.to_str().expect("utf-8 sibling path"),
            ],
        );
    }
}

pub(super) async fn dual_worktree_gateway(
    primary: PathBuf,
    sibling: PathBuf,
) -> (SocketAddr, WorkspaceRootLog) {
    let roots: WorkspaceRootLog = Arc::default();
    let recorded = Arc::clone(&roots);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let primary = primary.clone();
            let sibling = sibling.clone();
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let _ = serve_dual_worktree(socket, primary, sibling, recorded).await;
            });
        }
    });
    (addr, roots)
}

async fn serve_dual_worktree(
    socket: TcpStream,
    primary: PathBuf,
    sibling: PathBuf,
    recorded: WorkspaceRootLog,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    let mut active_root = primary.clone();
    while let Some(msg) = framed.next().await {
        match msg? {
            WireMessage::SyncProbeRequest(req) => {
                recorded
                    .lock()
                    .unwrap()
                    .push(req.client_workspace_root.clone());
                let missing: Vec<String> =
                    req.files.iter().map(|f| f.relative_path.clone()).collect();
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing,
                    }))
                    .await?;
            }
            WireMessage::SyncRequest(req) => {
                let files_updated = req.files.len();
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated,
                        files_deleted: 0,
                        bytes_transferred: 64,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(req) => {
                active_root = std::fs::canonicalize(&req.client_workspace_root)
                    .unwrap_or_else(|_| PathBuf::from(&req.client_workspace_root));
                recorded
                    .lock()
                    .unwrap()
                    .push(req.client_workspace_root.clone());
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        server_workspace_root: req.client_workspace_root,
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: None,
                        index_gated: false,
                        capabilities: None,
                    }))
                    .await?;
            }
            WireMessage::ReadFileRequest(req) => {
                let content = std::fs::read(&req.path).ok();
                let error = content.is_none().then(|| "no such file".to_string());
                framed
                    .send(WireMessage::ReadFileResponse(ReadFileResponse {
                        path: req.path,
                        content,
                        truncated: false,
                        is_executable: Some(false),
                        error,
                    }))
                    .await?;
            }
            WireMessage::ExecRequest(req) => {
                recorded
                    .lock()
                    .unwrap()
                    .push(req.client_workspace_root.clone());
                framed
                    .send(WireMessage::ExecExit(ExecExit {
                        exit_code: Some(127),
                        duration_ms: 1,
                        server_workspace_root: req.client_workspace_root,
                        timed_out: false,
                        error: None,
                        usage: None,
                        platform: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(json) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let Some(id) = value.get("id").cloned() else {
                    continue;
                };
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let is_sibling = active_root == sibling;
                let root = if is_sibling { &sibling } else { &primary };
                let result = lsp_answer_for_worktree(root, is_sibling, method, &params);
                let reply = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                framed
                    .send(WireMessage::LspPayload(reply.to_string()))
                    .await?;
            }
            WireMessage::StatusRequest => {
                framed
                    .send(WireMessage::StatusResponse(StatusResponse {
                        server_pid: std::process::id(),
                        detected_engines: vec!["rust".to_string()],
                        ..StatusResponse::default()
                    }))
                    .await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::Auth(_) | WireMessage::Pong => {}
            WireMessage::Disconnect { .. } => break,
            other => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: format!("unhandled mock message: {other:?}"),
                    })
                    .await;
                break;
            }
        }
    }
    Ok(())
}

fn lsp_answer_for_worktree(
    root: &Path,
    is_sibling: bool,
    method: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let lib = root.join("src/lib.rs");
    let uri = url::Url::from_file_path(&lib)
        .expect("file uri")
        .to_string();
    let range = |line: u32, start: u32, end: u32| {
        serde_json::json!({
            "start": { "line": line, "character": start },
            "end": { "line": line, "character": end }
        })
    };
    let span = |start_line: u32, start_char: u32, end_line: u32, end_char: u32| {
        serde_json::json!({
            "start": { "line": start_line, "character": start_char },
            "end": { "line": end_line, "character": end_char }
        })
    };
    let impl_name = if is_sibling {
        "SiblingImpl"
    } else {
        "PrimaryImpl"
    };
    let helper_name = if is_sibling {
        "helper_sibling"
    } else {
        "helper_primary"
    };
    let caller_name = if is_sibling {
        "caller_sibling"
    } else {
        "caller_primary"
    };
    match method {
        "initialize" => serde_json::json!({ "capabilities": { "hoverProvider": true } }),
        "workspace/symbol" => {
            let mut items = vec![
                serde_json::json!({ "name": "TargetTrait", "kind": 11, "location": { "uri": uri, "range": range(1, 10, 21) } }),
                serde_json::json!({ "name": impl_name, "kind": 23, "location": { "uri": uri, "range": range(2, 11, 22) } }),
                serde_json::json!({ "name": "target_fn", "kind": 12, "location": { "uri": uri, "range": range(4, 7, 16) } }),
                serde_json::json!({ "name": helper_name, "kind": 12, "location": { "uri": uri, "range": range(7, 7, 21) } }),
                serde_json::json!({ "name": caller_name, "kind": 12, "location": { "uri": uri, "range": range(10, 7, 21) } }),
            ];
            if is_sibling {
                items.push(serde_json::json!({
                    "name": "sibling_only_fn",
                    "kind": 12,
                    "location": { "uri": uri, "range": range(14, 7, 22) }
                }));
            }
            serde_json::Value::Array(items)
        }
        "textDocument/documentSymbol" => {
            let mut docs = vec![
                serde_json::json!({ "name": "TargetTrait", "kind": 11, "range": range(1, 0, 38), "selectionRange": range(1, 10, 21) }),
                serde_json::json!({ "name": impl_name, "kind": 23, "range": range(2, 0, 23), "selectionRange": range(2, 11, 22) }),
                serde_json::json!({ "name": "target_fn", "kind": 12, "range": span(4, 0, 6, 1), "selectionRange": range(4, 7, 16) }),
                serde_json::json!({ "name": helper_name, "kind": 12, "range": span(7, 0, 9, 1), "selectionRange": range(7, 7, 21) }),
                serde_json::json!({ "name": caller_name, "kind": 12, "range": span(10, 0, 13, 1), "selectionRange": range(10, 7, 21) }),
            ];
            if is_sibling {
                docs.push(serde_json::json!({
                    "name": "sibling_only_fn",
                    "kind": 12,
                    "range": span(14, 0, 16, 1),
                    "selectionRange": range(14, 7, 22)
                }));
            }
            serde_json::Value::Array(docs)
        }
        "textDocument/definition" => {
            let line = params
                .pointer("/position/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(4) as u32;
            let (def_line, start, end) = match line {
                1 => (1, 10, 21),
                14 if is_sibling => (14, 7, 22),
                _ => (4, 7, 16),
            };
            serde_json::json!([{ "uri": uri, "range": range(def_line, start, end) }])
        }
        "textDocument/references" => serde_json::json!([
            { "uri": uri, "range": range(4, 7, 16) },
            { "uri": uri, "range": range(11, 24, 33) }
        ]),
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "target_fn",
            "kind": 12,
            "uri": uri,
            "range": range(4, 0, 28),
            "selectionRange": range(4, 7, 16)
        }]),
        "callHierarchy/incomingCalls" => {
            if params.pointer("/item/name").and_then(|n| n.as_str()) != Some("target_fn") {
                return serde_json::json!([]);
            }
            serde_json::json!([{
                "from": {
                    "name": caller_name,
                    "kind": 12,
                    "uri": uri,
                    "range": range(10, 0, 28),
                    "selectionRange": range(10, 7, 21)
                },
                "fromRanges": [range(11, 24, 33)]
            }])
        }
        "callHierarchy/outgoingCalls" => {
            if params.pointer("/item/name").and_then(|n| n.as_str()) != Some("target_fn") {
                return serde_json::json!([]);
            }
            serde_json::json!([{
                "to": {
                    "name": helper_name,
                    "kind": 12,
                    "uri": uri,
                    "range": range(7, 0, 28),
                    "selectionRange": range(7, 7, 21)
                },
                "fromRanges": [range(5, 4, 18)]
            }])
        }
        "textDocument/hover" => serde_json::json!({
            "contents": {
                "kind": "markdown",
                "value": if is_sibling { "fn target_fn() -> SiblingReturn" } else { "fn target_fn() -> PrimaryReturn" }
            }
        }),
        "textDocument/implementation" => {
            serde_json::json!([{ "uri": uri, "range": range(3, 0, 34) }])
        }
        _ => serde_json::Value::Null,
    }
}
