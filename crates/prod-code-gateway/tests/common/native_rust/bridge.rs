/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Transport-only bridge from the public MCP client to the real native dispatcher.
//! Client and server share the fixture directory: probes verify disk hashes, deltas
//! verify disk bytes, and neither writes nor semantic replies are synthesized here.

use super::{Fixture, Session};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use prod_code_gateway::admission::Admission;
use prod_code_protocol::{
    HandshakeResponse, PathTranslator, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
    content_hash, negotiate_protocol_version,
};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tokio_util::codec::Framed;

#[derive(Debug)]
pub struct Answer {
    pub session: u64,
    pub validation: bool,
    pub method: String,
    pub request: Value,
    pub response: Value,
}

pub struct Bridge {
    listener: TcpListener,
    pub addr: SocketAddr,
}

impl Bridge {
    pub async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self {
            addr: listener.local_addr().unwrap(),
            listener,
        }
    }

    // No detached tasks: join! drives this server beside the consumer. Stop closes
    // pooled connections too; each connection unregisters its view before returning.
    pub async fn run(self, fixture: &Fixture, mut stop: watch::Receiver<bool>) -> Vec<Answer> {
        let mut connections = FuturesUnordered::new();
        let mut answers = Vec::new();
        let mut next = 100;
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                socket = self.listener.accept() => {
                    let (socket, _) = socket.unwrap();
                    connections.push(serve(socket, fixture, next, stop.clone()));
                    next += 1;
                }
                Some(result) = connections.next(), if !connections.is_empty() => {
                    answers.extend(result);
                }
            }
        }
        while let Some(result) = connections.next().await {
            answers.extend(result);
        }
        answers
    }
}

async fn serve(
    socket: TcpStream,
    fixture: &Fixture,
    id: u64,
    mut stop: watch::Receiver<bool>,
) -> Vec<Answer> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    let mut session: Option<Session> = None;
    let mut validation = false;
    let mut answers = Vec::new();
    loop {
        let message = tokio::select! {
            _ = stop.changed() => break,
            message = framed.next() => match message {
                Some(Ok(message)) => message,
                None => break,
                other => panic!("wire decode: {other:?}"),
            }
        };
        match message {
            WireMessage::SyncProbeRequest(req) => {
                assert_eq!(
                    std::path::Path::new(&req.client_workspace_root),
                    fixture.workspace.root
                );
                for file in &req.files {
                    let bytes =
                        std::fs::read(fixture.workspace.root.join(&file.relative_path)).unwrap();
                    assert_eq!(bytes.len() as u64, file.size);
                    assert_eq!(content_hash(&bytes), file.hash);
                }
                println!(
                    "BOUNDARY probe session={id} verified_files={}",
                    req.files.len()
                );
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: Vec::new(),
                    }))
                    .await
                    .unwrap();
            }
            WireMessage::SyncRequest(req) => {
                assert_eq!(
                    std::path::Path::new(&req.client_workspace_root),
                    fixture.workspace.root
                );
                for file in &req.files {
                    let disk = std::fs::read(fixture.workspace.root.join(&file.relative_path)).ok();
                    assert_eq!(disk, file.content);
                }
                println!(
                    "BOUNDARY sync session={id} verified_deltas={}",
                    req.files.len()
                );
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 0,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await
                    .unwrap();
            }
            WireMessage::HandshakeRequest(req) => {
                assert!(session.is_none());
                assert_eq!(
                    std::path::Path::new(&req.client_workspace_root),
                    fixture.workspace.root
                );
                assert_eq!(req.preferred_engine.as_deref(), Some("rust"));
                validation = req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION);
                assert!(validation || req.purpose.is_none());
                let mut native = fixture.session(id).await;
                // Identity translation is deliberate: both peers use the same actual
                // fixture root, not the endpoint tests' virtual /client root.
                native.translator =
                    PathTranslator::new(&req.client_workspace_root, &req.client_workspace_root);
                if validation {
                    native.view.workspace = fixture
                        .workspace
                        .validation_view(&Arc::new(Admission::unbounded()))
                        .await
                        .unwrap();
                    assert!(!Arc::ptr_eq(
                        native.view.workspace.rust_engine.as_ref().unwrap(),
                        fixture.workspace.rust_engine.as_ref().unwrap(),
                    ));
                }
                println!(
                    "BOUNDARY handshake session={id} validation={validation} native=true backend=false"
                );
                session = Some(native);
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: negotiate_protocol_version(&req).unwrap(),
                        server_pid: std::process::id(),
                        session_id: id,
                        server_workspace_root: req.client_workspace_root,
                        detected_engine: "rust".into(),
                        stale_paths: Vec::new(),
                        engine_age_ms: None,
                        index_gated: false,
                        capabilities: None,
                    }))
                    .await
                    .unwrap();
            }
            WireMessage::LspPayload(payload) => {
                let request: Value = serde_json::from_str(&payload).unwrap();
                let native = session.as_mut().expect("handshake before LSP");
                native.send(request.clone()).await;
                if request.get("id").is_some() {
                    let response = native
                        .receive()
                        .await
                        .expect("production dispatch must answer");
                    assert_eq!(response["id"], request["id"]);
                    answers.push(Answer {
                        session: id,
                        validation,
                        method: request["method"].as_str().unwrap().into(),
                        request,
                        response: response.clone(),
                    });
                    framed
                        .send(WireMessage::LspPayload(response.to_string()))
                        .await
                        .unwrap();
                }
            }
            WireMessage::Ping => framed.send(WireMessage::Pong).await.unwrap(),
            WireMessage::Disconnect { .. } => break,
            other => panic!("unimplemented transport boundary: {other:?}"),
        }
    }
    if let Some(native) = session {
        fixture.finish(vec![native]).await;
        println!("BOUNDARY unregister session={id}");
    }
    answers
}
