/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::answers;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, ProdCodeCodec, ReadFileResponse, ShadowRunResponse, SyncProbeResponse,
    SyncResponse, WireMessage, negotiate_protocol_version,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// The key of an answer that is an error response rather than a result: its value is the
/// JSON-RPC `error` object.
pub const LSP_ERROR: &str = "prod-code/lsp-error";

/// Answers one LSP request: the JSON-RPC method and its params, in, the `result` out. An
/// answer made by [`answers::failure`] goes back as the JSON-RPC `error` instead.
pub type Answer = Arc<dyn Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync>;

/// A gateway that syncs nothing, loads nothing, and answers from a script.
pub struct ScriptedGateway {
    addr: SocketAddr,
    calls: Arc<AtomicUsize>,
}

impl ScriptedGateway {
    /// Starts one on an ephemeral port. It lives as long as the test process.
    pub async fn start<F>(answer: F) -> Self
    where
        F: Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    {
        Self::start_arc(Arc::new(answer)).await
    }

    /// Starts one from an already shared closure, for a script that keeps state of its own.
    pub async fn start_arc(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let answer = Arc::clone(&answer);
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    let _ = serve(socket, answer, counted).await;
                });
            }
        });
        Self { addr, calls }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many LSP requests the script has been asked, `initialize` aside. A tool that asks
    /// the analyzer twice about the same place is a bug this number catches.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

async fn serve(socket: TcpStream, answer: Answer, calls: Arc<AtomicUsize>) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(message) = framed.next().await {
        match message? {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root.clone(),
                        seeded: false,
                        files_deleted: 0,
                        // Nothing is missing: the gateway pretends it already holds the tree,
                        // so a test never depends on what the client decides to upload.
                        missing: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::SyncRequest(req) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root.clone(),
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 0,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let protocol_version = match negotiate_protocol_version(&req) {
                    Ok(version) => version,
                    Err(err) => {
                        framed
                            .send(WireMessage::Disconnect {
                                reason: format!("protocol negotiation failed: {err}"),
                            })
                            .await?;
                        return Ok(());
                    }
                };
                // Shown to the script as `prod-code/handshake`, so a test can see which engine
                // a session asked for (`purpose`); an answer with `engine_age_ms` says how long
                // ago the engine was loaded (#381), any other answer leaves it unsaid.
                let said = answer(
                    "prod-code/handshake",
                    &serde_json::json!({ "purpose": req.purpose.clone() }),
                );
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version,
                        server_pid: std::process::id(),
                        session_id: 1,
                        // The same path on both sides, so `PathTranslator` is the identity and
                        // a script can answer with the test's own paths.
                        server_workspace_root: req.client_workspace_root.clone(),
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: said.get("engine_age_ms").and_then(|v| v.as_u64()),
                        index_gated: said
                            .get("index_gated")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        capabilities: None,
                    }))
                    .await?;
            }
            // A file that lives only on the node (a dependency's source) is read through the
            // script as the pseudo-method `prod-code/readFile`: a string is the file's text,
            // anything else means it cannot be read.
            WireMessage::ReadFileRequest(req) => {
                let text = answer(
                    "prod-code/readFile",
                    &serde_json::json!({ "path": req.path.clone() }),
                );
                let (content, error) = match text.as_str() {
                    Some(text) => (Some(text.as_bytes().to_vec()), None),
                    None => (None, Some(format!("no such file: {}", req.path))),
                };
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
            WireMessage::LspPayload(json) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let Some(id) = value.get("id").cloned() else {
                    // A notification — didOpen, didChange, initialized — is shown to the script,
                    // which may keep the text it carries, and gets no answer.
                    if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                        let params = value
                            .get("params")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let _ = answer(method, &params);
                    }
                    continue;
                };
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result = if method == "initialize" {
                    serde_json::json!({ "capabilities": { "hoverProvider": true } })
                } else {
                    calls.fetch_add(1, Ordering::Relaxed);
                    answer(method, &params)
                };
                if let Some(target_addr) = result.get("redirect").and_then(|v| v.as_str()) {
                    let reason = result
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    framed
                        .send(WireMessage::Redirect {
                            target_addr: target_addr.to_string(),
                            reason,
                        })
                        .await?;
                    break;
                }
                // A request of the server's own that the script wants passed on before the
                // answer (`prod-code/server-request` returns it; it is given the question's id),
                // as an older gateway passed on gopls's (#391).
                if prod_code_protocol::readiness::needs_index(method) {
                    let mut request = answer(
                        "prod-code/server-request",
                        &serde_json::json!({ "method": method }),
                    );
                    if request.get("method").is_some() {
                        request["id"] = id.clone();
                        framed
                            .send(WireMessage::LspPayload(request.to_string()))
                            .await?;
                    }
                }
                // An index question the script says the server answered while still indexing
                // (`prod-code/busy` returns the work) comes with the gateway's note (#391).
                if prod_code_protocol::readiness::needs_index(method) {
                    let busy = answer("prod-code/busy", &serde_json::json!({ "method": method }));
                    if busy.get("title").is_some() {
                        let note = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
                            "params": busy
                        });
                        framed
                            .send(WireMessage::LspPayload(note.to_string()))
                            .await?;
                    }
                }
                let response = match result
                    .get(answers::FAILURE)
                    .or_else(|| result.get(answers::RPC_ERROR))
                    .or_else(|| result.get(LSP_ERROR))
                {
                    Some(error) => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error })
                    }
                    None => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                };
                framed
                    .send(WireMessage::LspPayload(response.to_string()))
                    .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                let said = answer(
                    "prod-code/shadow-run",
                    &serde_json::json!({
                        "command": req.command,
                    }),
                );
                let (results, error) = if let Some(err) = said.get("error").and_then(|e| e.as_str())
                {
                    (Vec::new(), Some(err.to_string()))
                } else if said.is_null() {
                    (
                        Vec::new(),
                        Some("shadow run not supported by scripted gateway".to_string()),
                    )
                } else {
                    let exit_code = said
                        .get("exit_code")
                        .and_then(|c| c.as_i64())
                        .map(|c| c as i32)
                        .unwrap_or(0);
                    let results = req
                        .hypotheses
                        .into_iter()
                        .map(|h| prod_code_protocol::ShadowHypothesisResult {
                            name: h.name,
                            exit_code: Some(exit_code),
                            duration_ms: 10,
                            timed_out: false,
                            error: None,
                            output_tail: Some(Vec::new()),
                            output_len: 0,
                        })
                        .collect();
                    (results, None)
                };
                let root = req.client_workspace_root.clone();
                framed
                    .send(WireMessage::ShadowRunResponse(ShadowRunResponse {
                        server_workspace_root: root,
                        mode: "mock".to_string(),
                        results,
                        error,
                    }))
                    .await?;
            }
            WireMessage::Disconnect { .. } => break,
            _ => {}
        }
    }
    Ok(())
}
