/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.
//!
//! The daemon is a library with a thin binary on top, so that the pieces a socket normally
//! stands in front of — the workspace manager, the dispatch, the language server backends —
//! can be exercised directly by tests.

pub mod admission;
pub mod backend;
pub mod cpp_index;
pub mod detect;
pub mod editor_proxy;
pub mod embed;
pub mod exec_shim;
pub mod memory;
mod metrics;
pub mod priming;
pub mod python_cache;
pub mod search;
pub mod shadow;
pub mod swift_cache;
pub mod ts_cache;
pub mod workspace;
pub mod sync;
pub use sync::*;
pub mod runner;
pub use runner::*;
pub(crate) mod lsp;
pub(crate) use lsp::*;
pub mod state;
pub use state::*;
pub mod seed_cache;
pub use seed_cache::*;
pub mod engines;
pub(crate) mod gossip;
pub(crate) mod janitor_task;
pub(crate) mod lsp_handlers;
pub mod server;

pub use engines::*;
pub(crate) use gossip::*;
pub(crate) use janitor_task::*;
pub(crate) use lsp_handlers::*;
pub use server::*;

pub use detect::detect_engine;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta,
    FileStamp, HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PathTranslator, PeerInfo,
    PlaceRequest, PlaceResponse, ProdCodeCodec, RemoteExecCommand, RemoteExecFormat,
    RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, RemoteExecStream, RemoteExecTestEvent,
    ScrubSecrets, StatusResponse, SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse,
    WireMessage, content_hash, negotiate_protocol_version, parse_cargo_json_event,
    parse_go_test_json_event,
    path::{file_uri, uri_or_path},
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};


/// Whether an LSP message is a request from the server (an id and a method), not a notification.
fn is_server_request(json: &str) -> bool {
    json.contains("\"id\"")
        && serde_json::from_str::<serde_json::Value>(json)
            .is_ok_and(|v| v.get("id").is_some() && v.get("method").is_some())
}

fn fallback_answers_request(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json).is_ok_and(|value| {
        value.get("id").is_some_and(|id| !id.is_null())
            && matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some(
                    "window/workDoneProgress/create"
                        | "workspace/configuration"
                        | "client/registerCapability"
                )
            )
    })
}

/// Sends the client the note an engine attached to an answer given while its server was still
/// loading or indexing, just before the answer, and takes it off the answer (#391).
async fn send_busy_note(resp: &mut serde_json::Value, out_tx: &SharedOutputSender) {
    let Some(busy) = resp
        .as_object_mut()
        .and_then(|o| o.remove(prod_code_protocol::readiness::BUSY_MEMBER))
    else {
        return;
    };
    let note = serde_json::json!({
        "jsonrpc": "2.0",
        "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
        "params": busy
    });
    let _ = out_tx.send(WireMessage::LspPayload(note.to_string())).await;
}

/// A managed (out-of-process) language server the gateway can talk LSP to.
/// Default wall-clock limit for a remote command when the client does not set one.
/// Apply batch file synchronization to server workspace storage.
pub async fn handle_client(
    stream: impl Into<AnyStream>,
    addr: impl std::fmt::Display,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(stream.into(), ProdCodeCodec::new());

    // A gateway with a token serves nothing, not even its status, to a connection that does
    // not open with it (#402).
    if let Some(expected) = state.auth_token.as_deref() {
        let first = tokio::time::timeout(AUTH_WAIT, framed.next())
            .await
            .ok()
            .flatten();
        match &first {
            Some(Ok(WireMessage::HttpProbe { method, path })) => {
                tracing::info!(%addr, %method, %path, "HTTP probe received on token-protected gateway port");
                let (status, payload) = if path == "/health" || path == "/healthz" {
                    (
                        200,
                        serde_json::json!({ "status": "ok", "service": "prod-code-gateway" }),
                    )
                } else {
                    (
                        426,
                        serde_json::json!({
                            "error": "protocol_mismatch",
                            "message": "Port 9400 serves prod-code remote code intelligence using a binary framing protocol (or TLS), not general HTTP."
                        }),
                    )
                };
                let _ = framed
                    .send(WireMessage::HttpResponse {
                        status,
                        content_type: "application/json".to_string(),
                        body: serde_json::to_string_pretty(&payload).unwrap_or_default(),
                    })
                    .await;
                return Ok(());
            }
            Some(Ok(WireMessage::Auth(token))) if token.matches(expected) => {}
            _ => {
                let presented = match &first {
                    Some(Ok(WireMessage::Auth(token))) => Some(token),
                    _ => None,
                };
                tracing::warn!(
                    %addr,
                    presented = presented.is_some(),
                    "🔒 [AUTH] closed a connection without the cluster's token"
                );
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: AUTH_REFUSED.to_string(),
                    })
                    .await;
                return Ok(());
            }
        }
    }

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            // A token sent to a gateway that requires none, or sent twice, changes nothing.
            WireMessage::Auth(_) => {}
            WireMessage::HttpProbe { method, path } => {
                tracing::info!(%addr, %method, %path, "HTTP probe/request received on gateway port");
                let (status, payload) = if path == "/health" || path == "/healthz" {
                    (
                        200,
                        serde_json::json!({ "status": "ok", "service": "prod-code-gateway" }),
                    )
                } else {
                    (
                        426,
                        serde_json::json!({
                            "error": "protocol_mismatch",
                            "message": "Port 9400 serves prod-code remote code intelligence using a binary framing protocol (or TLS), not general HTTP. Connect using the prod-code CLI or MCP server. For health checks, GET /health is supported."
                        }),
                    )
                };
                let _ = framed
                    .send(WireMessage::HttpResponse {
                        status,
                        content_type: "application/json".to_string(),
                        body: serde_json::to_string_pretty(&payload).unwrap_or_default(),
                    })
                    .await;
                return Ok(());
            }
            WireMessage::StatusRequest => {
                let status = state.status().await;
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::Gossip(gossip) => {
                state.absorb_gossip(gossip).await;
                let own = state.own_gossip().await;
                framed.send(WireMessage::Gossip(own)).await?;
            }
            WireMessage::ClusterRequest => {
                let view = state.cluster_view().await;
                framed.send(WireMessage::ClusterResponse(view)).await?;
            }
            WireMessage::PlaceRequest(req) => {
                let resp = state.place(&req).await;
                framed.send(WireMessage::PlaceResponse(resp)).await?;
            }
            WireMessage::MetricsRequest(req) => {
                let node = state.advertise.read().await.clone();
                let resp = state.metrics.summary(&node, req.since_secs);
                framed.send(WireMessage::MetricsResponse(resp)).await?;
            }
            WireMessage::SyncRequest(req) => {
                let workspace = workspace::server_workspace_path(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let touched: Vec<String> =
                    req.files.iter().map(|f| f.relative_path.clone()).collect();
                let resp = apply_sync_with_metrics(
                    &state.storage_root,
                    &state.workspace_manager,
                    Some(&state.metrics),
                    req,
                )
                .await;
                // The files an agent is editing are the ones it validates next: warm them now
                // (#233).
                let synced_rust = priming::synced_rust_files(&workspace, &touched);
                if !synced_rust.is_empty()
                    && let Some(loaded) = state.workspace_manager.get_loaded(&workspace).await
                    && loaded.rust_engine.is_some()
                {
                    // Validation runs on its own engine: that is the one to warm. Loading it
                    // warms the newest files, these among them.
                    let workspace = workspace.clone();
                    let admission = Arc::clone(state.workspace_manager.admission());
                    tokio::spawn(async move {
                        if let Ok(view) = loaded.validation_view(&admission).await
                            && let Some(engine) = view.rust_engine.clone()
                        {
                            priming::warm_in_background(engine, workspace, synced_rust);
                        }
                    });
                }
                // The search index is kept current by what the sync wrote, so a query never
                // has to walk the tree.
                state.search_indexes.invalidate(&workspace, touched);
                framed.send(WireMessage::SyncResponse(resp)).await?;
            }
            WireMessage::SyncProbeRequest(req) => {
                let resp =
                    apply_sync_probe(&state.storage_root, &state.workspace_manager, req).await;
                framed.send(WireMessage::SyncProbeResponse(resp)).await?;
            }
            WireMessage::ExecRequest(req) => {
                run_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
                )
                .await?;
            }
            WireMessage::RemoteExecRequest(req) => {
                run_remote_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
                )
                .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                shadow::run_shadow(&state, &mut framed, req).await?;
            }
            WireMessage::SearchRequest(req) => {
                let resp = {
                    let state = Arc::clone(&state);
                    tokio::task::spawn_blocking(move || {
                        search::run_search(&state.search_indexes, &state.storage_root, &req)
                    })
                    .await?
                };
                framed.send(WireMessage::SearchResponse(resp)).await?;
            }
            WireMessage::ReadFileRequest(req) => {
                let resp = read_server_file(&state.storage_root, &req);
                framed.send(WireMessage::ReadFileResponse(resp)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let protocol_version = match negotiate_protocol_version(&req) {
                    Ok(version) => version,
                    Err(err) => {
                        let reason = format!("gateway refused protocol negotiation: {err}");
                        tracing::warn!(reason, "refusing incompatible handshake");
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                let _active_session = ActiveSession::start(&state.active_sessions);
                let session_capabilities = prod_code_protocol::negotiate_capabilities(
                    req.capabilities.as_ref(),
                    &prod_code_protocol::default_server_capabilities(),
                );

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let server_workspace = workspace::resolve_server_workspace(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let server_workspace_str = server_workspace.to_string_lossy().to_string();
                workspace::touch_last_used(&server_workspace);

                // A nested project of another language (engine_subpath) gets its own engine
                // rooted there; sync and path translation stay on the checkout root.
                let engine_root = match req.engine_subpath.as_deref() {
                    Some(sub)
                        if !sub.is_empty()
                            && !sub.starts_with('/')
                            && !sub.split('/').any(|c| c == "..")
                            && server_workspace.join(sub).is_dir() =>
                    {
                        server_workspace.join(sub)
                    }
                    Some(sub) if !sub.is_empty() => {
                        tracing::warn!(subpath = sub, "engine_subpath ignored (missing or unsafe)");
                        server_workspace.clone()
                    }
                    _ => server_workspace.clone(),
                };

                let engine_kind =
                    detect::resolve_engine(&engine_root, req.preferred_engine.as_deref());
                let engine = engine_kind.as_str();
                let supports_redirects = req
                    .capabilities
                    .as_ref()
                    .is_some_and(|capabilities| capabilities.redirects);
                if !state.serves_engine(engine) {
                    if supports_redirects && req.redirect_count < 2 {
                        let view = state.cluster_view().await;
                        if let Some(target) = view
                            .nodes
                            .iter()
                            .find(|n| n.alive && cluster_supports_engine(&n.status, engine))
                        {
                            tracing::info!(
                                engine,
                                target = %target.addr,
                                "redirecting client to cluster node serving engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: target.addr.clone(),
                                    reason: Some(format!(
                                        "engine {engine} is served by {}",
                                        target.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                    let reason = format!(
                        "engine {engine} is not served by this node (--engines {}); pick a node that lists it",
                        state.engine_allowlist.join(",")
                    );
                    tracing::warn!(
                        client_root = %req.client_workspace_root,
                        engine,
                        "refusing handshake: engine not served here"
                    );
                    framed.send(WireMessage::Disconnect { reason }).await?;
                    return Ok(());
                }

                // Roadmap 5.1: If this workspace is not already loaded locally, but another live cluster
                // node has it loaded warm, transparently redirect the client there.
                let is_loaded_locally = state
                    .workspace_manager
                    .get_loaded(&server_workspace)
                    .await
                    .is_some();
                if supports_redirects && req.redirect_count < 3 {
                    let view = state.cluster_view().await;
                    let own_addr = state.advertise.read().await.clone();
                    let ws_name = req
                        .base_workspace_name
                        .as_deref()
                        .unwrap_or(&req.client_workspace_root);
                    if !is_loaded_locally && req.redirect_count == 0 {
                        if let Some(holder) = view.nodes.iter().find(|n| {
                            n.alive
                                && n.addr != own_addr
                                && cluster_supports_engine(&n.status, engine)
                                && n.workspaces.iter().any(|w| w.name == ws_name)
                        }) {
                            tracing::info!(
                                workspace = ws_name,
                                target = %holder.addr,
                                "transparently redirecting client to node with warm workspace engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: holder.addr.clone(),
                                    reason: Some(format!(
                                        "workspace {ws_name} is already warm on {}",
                                        holder.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }

                    // Roadmap 5.3: Dynamic workload rebalancing and resource pressure load shedding.
                    // If this gateway is under resource pressure or congested, and another live node is roomy,
                    // redirect this workspace connection to the quietest roomiest node!
                    // If already loaded locally, require severe congestion or hard pressure to avoid unnecessary churn.
                    let current_status = state.status().await;
                    let under_pressure = current_status.host.pressure().is_some();
                    let congested = if is_loaded_locally {
                        under_pressure || current_status.congestion_score() >= 1.5
                    } else {
                        under_pressure || current_status.congestion_score() >= 1.2
                    };
                    if congested {
                        let own_score = current_status.congestion_score();
                        if let Some(roomy) = view
                            .nodes
                            .iter()
                            .filter(|n| {
                                n.alive
                                    && n.addr != own_addr
                                    && cluster_supports_engine(&n.status, engine)
                                    && n.status.host.pressure().is_none()
                                    && n.status.congestion_score() < own_score * 0.6
                            })
                            .min_by(|a, b| {
                                a.status
                                    .congestion_score()
                                    .partial_cmp(&b.status.congestion_score())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                        {
                            tracing::info!(
                                workspace = ws_name,
                                target = %roomy.addr,
                                loaded = is_loaded_locally,
                                "transparently redirecting client from congested gateway to roomier node"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: roomy.addr.clone(),
                                    reason: Some(format!(
                                        "node {own_addr} is congested (score {:.2}); redirected to roomier node {}",
                                        own_score,
                                        roomy.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                }

                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // An editor gets the language server it would run locally, a process of its own
                // on this node (#332); without one here, the shared engines answer it.
                if req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR)
                    && editor_proxy::enabled()
                    && let Some(command) =
                        editor_proxy::server_command_for_workspace(engine, &server_workspace)
                {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version,
                            server_pid: state.server_pid,
                            session_id,
                            server_workspace_root: server_workspace_str.clone(),
                            detected_engine: engine.to_string(),
                            stale_paths: workspace::stale_paths(&server_workspace),
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: Some(session_capabilities.clone()),
                        }))
                        .await?;
                    let outcome = editor_proxy::run(
                        framed,
                        translator,
                        command,
                        &engine_root,
                        &state.workspace_manager.editor_servers,
                        session_id,
                    )
                    .await;
                    return outcome;
                }

                // Attach to shared workspace using leader-follower coalescing. A load refused
                // for capacity (#433), or failed, is told to the client, which says why.
                let shared_ws = match state
                    .workspace_manager
                    .get_or_load(&engine_root, engine)
                    .await
                {
                    Ok(shared_ws) => shared_ws,
                    Err(err) => {
                        let reason = format!("{err:#}");
                        tracing::warn!(
                            client_root = %req.client_workspace_root,
                            engine,
                            reason,
                            "refusing handshake: the engine could not be loaded"
                        );
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let engine_age_ms = shared_ws.loaded_at.elapsed().as_millis() as u64;
                // The in-process Rust engine answers from a complete analysis once loaded; gopls
                // and the servers whose readiness is known are waited for (#391).
                let index_gated = shared_ws.rust_engine.is_some()
                    || shared_ws.go_engine.is_some()
                    || shared_ws
                        .generic_engine
                        .as_ref()
                        .is_some_and(|engine| engine.readiness_known());

                let validation =
                    req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION);
                let generic_validation_session = if validation && shared_ws.generic_engine.is_some()
                {
                    Some(Arc::clone(&shared_ws.generic_validation_session))
                } else {
                    None
                };
                let mut session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;
                // A session that only validates proposed texts runs on the workspace's second
                // engine, so its overlays never invalidate the main one (#73).
                let _generic_validation_session = if let Some(serial) = generic_validation_session {
                    Some(serial.lock_owned().await)
                } else {
                    None
                };
                if validation {
                    let validation_view = session_view
                        .accounted
                        .validation_view(state.workspace_manager.admission())
                        .await;
                    match validation_view {
                        Ok(view) => session_view.workspace = view,
                        Err(err) => {
                            let is_capacity = err
                                .downcast_ref::<crate::admission::CapacityRefused>()
                                .is_some()
                                || err.root_cause().is::<crate::admission::CapacityRefused>()
                                || err.to_string().contains("capacity:");

                            if is_capacity && req.redirect_count < 2 {
                                let view = state.cluster_view().await;
                                let own_addr = state.advertise.read().await.clone();
                                let ws_name = req
                                    .base_workspace_name
                                    .as_deref()
                                    .unwrap_or(&req.client_workspace_root);
                                let required_headroom =
                                    state.workspace_manager.admission().reserve_for(engine);
                                let target = view
                                    .nodes
                                    .iter()
                                    .filter(|n| {
                                        n.alive
                                            && !n.addr.is_empty()
                                            && n.addr != own_addr
                                            && n.addr != view.this_node
                                            && cluster_supports_engine(&n.status, engine)
                                            && n.status.host.pressure().is_none()
                                            && n.status.host.memory_available_bytes.is_none_or(
                                                |available| available >= required_headroom,
                                            )
                                            && (req.redirect_count == 0
                                                || !n.workspaces.iter().any(|w| w.name == ws_name))
                                    })
                                    .max_by_key(|n| {
                                        n.status.host.memory_available_bytes.unwrap_or(0)
                                    });

                                if let Some(target) = target {
                                    tracing::info!(
                                        session_id,
                                        engine,
                                        target = %target.addr,
                                        "redirecting validation session under gateway memory pressure"
                                    );
                                    state
                                        .workspace_manager
                                        .unregister_session_view(session_view)
                                        .await;
                                    let _ = framed
                                        .send(WireMessage::Redirect {
                                            target_addr: target.addr.clone(),
                                            reason: Some(format!(
                                                "gateway memory pressure; validating {engine} on {}",
                                                target.addr
                                            )),
                                        })
                                        .await;
                                    return Ok(());
                                }
                            }

                            let reason = format!("private validation engine unavailable: {err:#}");
                            tracing::warn!(
                                session_id,
                                engine,
                                reason,
                                "refusing validation handshake"
                            );
                            state
                                .workspace_manager
                                .unregister_session_view(session_view)
                                .await;
                            framed.send(WireMessage::Disconnect { reason }).await?;
                            return Ok(());
                        }
                    }
                }

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine_root = %engine_root.display(),
                    engine,
                    is_single_owner = session_view.is_single_owner(),
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner()
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                        stale_paths: workspace::stale_paths(&server_workspace),
                        engine_age_ms: Some(engine_age_ms),
                        index_gated,
                        capabilities: Some(session_capabilities),
                    }))
                    .await?;

                // Session loop for streaming LSP and control messages
                let meta = Arc::new(SessionMeta {
                    session_id,
                    client_name: req.client_name.clone(),
                    agent: req
                        .client_agent
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    host: req
                        .client_host
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    client_addr: addr.to_string(),
                    workspace: engine_root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    engine: engine.to_string(),
                    engine_root: engine_root.clone(),
                    storage_root: state.storage_root.clone(),
                    metrics: Arc::clone(&state.metrics),
                    editor: req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR),
                    edits: Arc::default(),
                });
                let session_res = run_session_loop(framed, &translator, &session_view, meta).await;

                state
                    .workspace_manager
                    .unregister_session_view(session_view)
                    .await;

                tracing::debug!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::debug!(%addr, reason, "Client disconnected cleanly");
                break;
            }
            other => {
                tracing::warn!(%addr, ?other, "Unexpected message before handshake");
            }
        }
    }

    Ok(())
}

async fn run_session_loop(
    framed: Framed<AnyStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: Arc<SessionMeta>,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    // rapidfire MPSC: every engine task sends, one writer drains in batches and flushes the
    // socket once per batch.
    let (raw_out_tx, mut out_rx) =
        rapidfire::mpsc::bounded::<SharedOutputFrame>(SHARED_OUTPUT_CAPACITY);
    let out_tx = SharedOutputSender::new(raw_out_tx, SHARED_OUTPUT_WRITE_BUDGET);

    // Requests in flight, keyed by JSON-RPC id, so every answer — whichever engine produced
    // it — becomes one metrics event with its duration.
    let pending: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let pending_writer = Arc::clone(&pending);
    let meta_writer = Arc::clone(&meta);
    let writer_output = out_tx.clone();
    let writer_handle = tokio::spawn(async move {
        let _lifetime = SharedWriterLifetime::start(writer_output);
        let mut batch: Vec<SharedOutputFrame> = Vec::with_capacity(SHARED_OUTPUT_BATCH);
        while out_rx
            .recv_many(&mut batch, SHARED_OUTPUT_BATCH)
            .await
            .is_ok()
        {
            let mut flush_deadline = None;
            for frame in batch.drain(..) {
                if tokio::time::Instant::now() >= frame.deadline {
                    anyhow::bail!("shared output frame expired while queued");
                }
                if let WireMessage::LspPayload(ref raw) = frame.message
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
                    && let Some(id) = val.get("id").filter(|i| !i.is_null())
                    && val.get("method").is_none()
                {
                    let key = id.to_string();
                    if let Some(req) = pending_writer.lock().await.remove(&key) {
                        let mut ev = metrics::Event::blank("lsp");
                        ev.session_id = meta_writer.session_id;
                        ev.client_name = meta_writer.client_name.clone();
                        ev.agent = meta_writer.agent.clone();
                        ev.host = meta_writer.host.clone();
                        ev.client_addr = meta_writer.client_addr.clone();
                        ev.workspace = meta_writer.workspace.clone();
                        ev.engine = meta_writer.engine.clone();
                        ev.method = req.method;
                        ev.file = req.file;
                        ev.line = req.line;
                        ev.col = req.col;
                        ev.duration_ms = req.start.elapsed().as_millis() as u64;
                        ev.ok = val.get("error").is_none();
                        ev.items = val
                            .get("result")
                            .map(|r| match r {
                                serde_json::Value::Array(a) => a.len() as u64,
                                serde_json::Value::Null => 0,
                                _ => 1,
                            })
                            .unwrap_or(0);
                        meta_writer.metrics.record(ev);
                    }
                }
                tokio::time::timeout_at(frame.deadline, socket_tx.feed(frame.message))
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket feed deadline elapsed"))??;
                flush_deadline = Some(match flush_deadline {
                    Some(current) => std::cmp::min(current, frame.deadline),
                    None => frame.deadline,
                });
            }
            if let Some(deadline) = flush_deadline {
                tokio::time::timeout_at(deadline, socket_tx.flush())
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket flush deadline elapsed"))??;
            }
        }
        Ok(())
    });
    // Install abort ownership before the session can reach another await. Cancellation of the
    // handler must cancel this exact writer, whose lifetime guard closes every producer queue.
    let mut writer = OwnedJoin::new(writer_handle);

    // gopls and the supervised servers answer their own requests (`window/workDoneProgress/create`,
    // `workspace/configuration`) in the engine; passed on, one carried the id of a client's
    // question and was taken for its answer (#391). Only their notifications go to the client.
    let engine_answers_requests =
        view.workspace.go_engine.is_some() || view.workspace.generic_engine.is_some();
    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    let mut rebalance_rx = view.accounted.subscribe_rebalance();
    let mut writer_finished = false;
    let mut session_result = Ok(());
    loop {
        tokio::select! {
            writer_result = writer.task_mut() => {
                writer.clear_finished();
                writer_finished = true;
                session_result = flatten_writer_result(writer_result);
                break;
            }
            client_msg_res = socket_rx.next() => {
                match on_client_message(client_msg_res, &out_tx, translator, view, &meta, &pending).await {
                    Flow::Next => continue,
                    Flow::Stop => break,
                }
            }

            rebalance_msg = rebalance_rx.recv() => {
                match rebalance_msg {
                    Ok((target_addr, reason)) => {
                        tracing::info!(
                            session_id = meta.session_id,
                            target = %target_addr,
                            ?reason,
                            "Session rebalanced: sending Redirect frame to active client"
                        );
                        let _ = out_tx.send(WireMessage::Redirect { target_addr, reason }).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }

            backend_msg = async {
                if let Some(ref mut rx) = backend_rx {
                    rx.recv().await
                } else {
                    futures_util::future::pending::<Result<String, tokio::sync::broadcast::error::RecvError>>().await
                }
            } => {
                match backend_msg {
                    Ok(server_lsp) => {
                        if (engine_answers_requests && is_server_request(&server_lsp))
                            || (!engine_answers_requests && fallback_answers_request(&server_lsp))
                        {
                            continue;
                        }
                        let client_lsp = translator.translate_lsp_to_client(&server_lsp);
                        if out_tx.send(WireMessage::LspPayload(client_lsp)).await.is_err() {
                            tracing::error!("Failed to send LSP message to client channel");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Session backend receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!("Backend worker broadcast closed");
                        break;
                    }
                }
            }
        }
    }
    out_tx.close();
    if !writer_finished {
        let teardown_deadline = tokio::time::Instant::now() + SHARED_OUTPUT_TEARDOWN_BUDGET;
        match tokio::time::timeout_at(teardown_deadline, writer.task_mut()).await {
            Ok(writer_result) => {
                writer.clear_finished();
                let writer_result = flatten_writer_result(writer_result);
                if session_result.is_ok() {
                    session_result = writer_result;
                }
            }
            Err(_) => {
                writer.abort();
                // Once aborted, await the exact task so no writer is detached. This is cleanup
                // after the single teardown deadline, not a second drain budget.
                let writer_result = writer.task_mut().await;
                writer.clear_finished();
                if let Err(error) = writer_result
                    && !error.is_cancelled()
                {
                    tracing::warn!(%error, "shared output writer failed while being aborted");
                }
                if session_result.is_ok() {
                    session_result = Err(anyhow::anyhow!(
                        "shared output writer exceeded teardown budget"
                    ));
                }
            }
        }
    }
    session_result
}

const SHARED_OUTPUT_CAPACITY: usize = 64;
const SHARED_OUTPUT_BATCH: usize = 64;
const SHARED_OUTPUT_WRITE_BUDGET: Duration = Duration::from_secs(2);
const SHARED_OUTPUT_TEARDOWN_BUDGET: Duration = Duration::from_secs(3);

#[doc(hidden)]
pub static ACTIVE_SHARED_OUTPUT_WRITERS: AtomicUsize = AtomicUsize::new(0);

struct SharedOutputFrame {
    message: WireMessage,
    deadline: tokio::time::Instant,
}

#[derive(Clone)]
struct SharedOutputSender {
    inner: rapidfire::mpsc::Sender<SharedOutputFrame>,
    write_budget: Duration,
}

#[derive(Debug)]
enum SharedOutputSendError {
    Closed,
    Deadline,
}

impl SharedOutputSender {
    fn new(inner: rapidfire::mpsc::Sender<SharedOutputFrame>, write_budget: Duration) -> Self {
        Self {
            inner,
            write_budget,
        }
    }

    async fn send(&self, message: WireMessage) -> std::result::Result<(), SharedOutputSendError> {
        let deadline = tokio::time::Instant::now() + self.write_budget;
        let frame = SharedOutputFrame { message, deadline };
        match tokio::time::timeout_at(deadline, self.inner.send(frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SharedOutputSendError::Closed),
            Err(_) => {
                // One expired producer expires the generation. This wakes every queue waiter and
                // prevents later notifications from repeatedly extending a dead client's life.
                self.close();
                Err(SharedOutputSendError::Deadline)
            }
        }
    }

    fn close(&self) {
        self.inner.close();
    }
}

struct SharedWriterLifetime {
    output: SharedOutputSender,
}

impl SharedWriterLifetime {
    fn start(output: SharedOutputSender) -> Self {
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_add(1, Ordering::Relaxed);
        Self { output }
    }
}

impl Drop for SharedWriterLifetime {
    fn drop(&mut self) {
        self.output.close();
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_sub(1, Ordering::Relaxed);
    }
}

struct OwnedJoin<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

impl<T> OwnedJoin<T> {
    fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn task_mut(&mut self) -> &mut tokio::task::JoinHandle<T> {
        self.task.as_mut().expect("owned task is live")
    }

    fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    fn clear_finished(&mut self) {
        let task = self.task.take().expect("owned task is live");
        debug_assert!(task.is_finished());
    }
}

impl<T> Drop for OwnedJoin<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn flatten_writer_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result.map_err(|error| anyhow::anyhow!("shared output writer task failed: {error}"))?
}

/// The capabilities an editor's `initialize` is answered with (#310).
///
/// A language server on the node advertises its own, except for document sync: the gateway
/// hands every change on as the document's full text, so the editor is asked for exactly that
/// whatever the server would take. The in-memory Rust engine advertises what it answers, with
/// the trigger characters rust-analyzer's own server uses; a workspace with neither only takes
/// documents.
fn editor_capabilities(server: Option<serde_json::Value>, rust: bool) -> serde_json::Value {
    let mut caps = match server {
        Some(caps) if caps.is_object() => caps,
        _ if rust => serde_json::json!({
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "implementationProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "renameProvider": true,
            "callHierarchyProvider": true,
            "completionProvider": {
                "triggerCharacters": [":", ".", "'", "("],
                "resolveProvider": true
            },
            "signatureHelpProvider": { "triggerCharacters": ["(", ",", "<"] },
            "inlayHintProvider": true,
            "documentHighlightProvider": true,
            "codeActionProvider": { "resolveProvider": true },
            "documentFormattingProvider": true
        }),
        _ => serde_json::json!({}),
    };
    let save = caps.pointer("/textDocumentSync/save").cloned();
    caps["textDocumentSync"] = serde_json::json!({ "openClose": true, "change": 1 });
    if let Some(save) = save {
        caps["textDocumentSync"]["save"] = save;
    }
    caps
}

/// What the session loop does once a client message has been handled.
enum Flow {
    /// Wait for the next message.
    Next,
    /// The client is gone or asked to disconnect: end the session.
    Stop,
}

/// Decode an LSP's zero-based position into the one-based coordinates used by the Rust engine.
///
/// LSP positions must be non-negative JSON integers. The engine's one-based API also means
/// that `u32::MAX` cannot be represented, so reject it rather than truncating or overflowing.
fn one_based_position(position: Option<&serde_json::Value>) -> Result<(u32, u32), &'static str> {
    let position = position.ok_or("position is required")?;
    let coordinate = |name| {
        position
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| value.checked_add(1))
            .ok_or("position coordinates must be non-negative integers below 4294967295")
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

/// Keep request metrics bounded even when a forwarded request has an arbitrary JSON shape.
fn metric_position(position: Option<&serde_json::Value>) -> (u32, u32) {
    one_based_position(position).unwrap_or((1, 1))
}

/// Validate only the Rust methods that consume LSP positions locally.
fn native_position_params(
    method: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<(), String> {
    let params = params.unwrap_or(&serde_json::Value::Null);
    match method {
        Some(
            "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/prepareCallHierarchy"
            | "prodCode/safeDelete"
            | "textDocument/rename",
        ) => one_based_position(params.get("position"))
            .map(|_| ())
            .map_err(str::to_owned),
        Some("callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls") => one_based_position(
            params
                .get("item")
                .and_then(|item| item.get("selectionRange"))
                .and_then(|range| range.get("start")),
        )
        .map(|_| ())
        .map_err(|reason| format!("item.selectionRange.start: {reason}")),
        Some("prodCode/structuralReplace") => params
            .get("position")
            .map(|position| {
                one_based_position(Some(position))
                    .map(|_| ())
                    .map_err(str::to_owned)
            })
            .unwrap_or(Ok(())),
        Some("prodCode/assists" | "prodCode/applyAssist") => {
            one_based_position(params.pointer("/range/start"))
                .map_err(|reason| format!("range.start: {reason}"))?;
            if let Some(end) = params.pointer("/range/end") {
                one_based_position(Some(end)).map_err(|reason| format!("range.end: {reason}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn send_invalid_params(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    id: &serde_json::Value,
    method: &str,
    reason: &str,
) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": format!("invalid params for {method}: {reason}"),
        }
    });
    let client_response = translator.translate_lsp_to_client(&response.to_string());
    let _ = out_tx.send(WireMessage::LspPayload(client_response)).await;
}

/// One message from the client: an LSP payload answered by the in-memory engine or forwarded
/// to the backend, a sync, a status request or a disconnect.
///
/// It lived inside the session loop's `tokio::select!`, where it was 1,400 lines of macro
/// input: rust-analyzer offers no refactoring inside a macro call, and every validation of the
/// file inferred it as one body (#86). Out here it is ordinary code.
async fn on_client_message(
    client_msg_res: Option<std::result::Result<WireMessage, std::io::Error>>,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    pending: &Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>>,
) -> Flow {
    match client_msg_res {
        Some(Ok(WireMessage::Ping)) => {
            let _ = out_tx.send(WireMessage::Pong).await;
        }
        Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
            let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
            tracing::debug!(
                payload_len = server_lsp.len(),
                single_owner = view.is_single_owner(),
                "Processing incoming LSP message"
            );

            // Inspect LSP message structure
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                let method = val.get("method").and_then(|m| m.as_str());
                let id = val.get("id").cloned();
                if view.workspace.rust_engine.is_some()
                    && let Err(reason) = native_position_params(method, val.get("params"))
                {
                    if let (Some(method), Some(id)) =
                        (method, id.as_ref().filter(|id| !id.is_null()))
                    {
                        send_invalid_params(out_tx, translator, id, method, &reason).await;
                    }
                    return Flow::Next;
                }
                if let (Some(m), Some(id_val)) = (method, &id)
                    && !id_val.is_null()
                    && m != "initialize"
                {
                    let params = val.get("params");
                    let uri = params
                        .and_then(|p| {
                            p.get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .or_else(|| p.get("item").and_then(|i| i.get("uri")))
                        })
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let path = uri_or_path(uri);
                    let file = path
                        .strip_prefix(&meta.engine_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    let pos = params.and_then(|p| {
                        p.get("position")
                            .or_else(|| p.get("range").and_then(|r| r.get("start")))
                            .or_else(|| {
                                p.get("item")
                                    .and_then(|item| item.get("selectionRange"))
                                    .and_then(|range| range.get("start"))
                            })
                    });
                    let (line, col) = metric_position(pos);
                    pending.lock().await.insert(
                        id_val.to_string(),
                        PendingRequest {
                            method: m.to_string(),
                            file,
                            line,
                            col,
                            start: Instant::now(),
                        },
                    );
                }

                // 1. Intercept "initialize": reply immediately with cached server capabilities
                if method == Some("initialize") {
                    let req_id = id.unwrap_or(serde_json::json!(1));
                    let caps = if let Some(ref go) = view.workspace.go_engine {
                        go.capabilities.read().await.clone()
                    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
                        generic_eng.capabilities.read().await.clone()
                    } else if let Some(ref backend) = view.workspace.backend {
                        backend.capabilities.read().await.clone()
                    } else {
                        None
                    };
                    let caps = editor_capabilities(caps, view.workspace.rust_engine.is_some());
                    let init_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "capabilities": caps,
                            "serverInfo": {
                                "name": "prod-code",
                                "version": env!("CARGO_PKG_VERSION")
                            }
                        }
                    });
                    let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                    let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
                    return Flow::Next;
                }

                // 2. Intercept "initialized": backend already initialized, consume without forwarding
                if method == Some("initialized") {
                    return Flow::Next;
                }

                // 3. Intercept "shutdown": reply cleanly
                if method == Some("shutdown") {
                    let req_id = id.unwrap_or(serde_json::json!(1));
                    let shutdown_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": null
                    });
                    let _ = out_tx
                        .send(WireMessage::LspPayload(shutdown_resp.to_string()))
                        .await;
                    return Flow::Next;
                }

                // 4. In-Memory RustEngine multi-core fast path: hover, definition, references, documentSymbol
                if let Some(ref engine_lock) = view.workspace.rust_engine {
                    match method {
                        Some("textDocument/hover") => {
                            if let Some(params) = val.get("params") {
                                lsp_hover(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/definition") => {
                            if let Some(params) = val.get("params") {
                                lsp_definition(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/references") => {
                            if let Some(params) = val.get("params") {
                                lsp_references(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/documentSymbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_document_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("workspace/symbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_workspace_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/assists") | Some("prodCode/applyAssist") => {
                            if let Some(params) = val.get("params") {
                                lsp_assists(
                                    out_tx,
                                    translator,
                                    view,
                                    method,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/safeDelete") => {
                            if let Some(params) = val.get("params") {
                                lsp_safe_delete(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some(
                            hm @ ("textDocument/prepareCallHierarchy"
                            | "callHierarchy/incomingCalls"
                            | "callHierarchy/outgoingCalls"
                            | "textDocument/implementation"
                            | "textDocument/diagnostic"),
                        ) => {
                            if let Some(params) = val.get("params") {
                                lsp_call_hierarchy(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    hm,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/rename") => {
                            if let Some(params) = val.get("params") {
                                lsp_rename(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/structuralReplace") => {
                            if let Some(params) = val.get("params") {
                                lsp_structural_replace(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some(m) if prod_code_engine_rust::editor::EDITOR_METHODS.contains(&m) => {
                            let params = val.get("params").cloned().unwrap_or_default();
                            lsp_editor_request(
                                out_tx,
                                translator,
                                view,
                                &id,
                                m,
                                params,
                                engine_lock,
                            );
                            return Flow::Next;
                        }
                        Some("textDocument/didOpen") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                if let Some(text) = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if view.is_single_owner() {
                                            if let Err(e) = engine
                                                .apply_file_change(&file_path, text.to_string())
                                            {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didOpen file change failed");
                                            }
                                            if let Ok(mut files) =
                                                view.direct_edit_open_files.lock()
                                            {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didOpen recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
                                    );
                                }
                            }
                        }
                        Some("textDocument/didChange") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                let first = params
                                    .get("contentChanges")
                                    .and_then(|c| c.as_array())
                                    .and_then(|arr| arr.first())
                                    .and_then(|c| c.get("text"))
                                    .and_then(|t| t.as_str());
                                if let Some(text) = first {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if view.is_single_owner() {
                                            if let Err(e) = engine
                                                .apply_file_change(&file_path, text.to_string())
                                            {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didChange file change failed");
                                            }
                                            if let Ok(mut files) =
                                                view.direct_edit_open_files.lock()
                                            {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didChange recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
                                    );
                                }
                            }
                        }
                        Some("textDocument/didClose") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                let mut engine = engine_lock.lock().await;
                                if view.is_single_owner() {
                                    if let Err(e) = engine.reload_file(&file_path) {
                                        tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didClose reload failed");
                                    }
                                    if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                        files.remove(&file_path);
                                    }
                                } else {
                                    if let Err(e) =
                                        engine.clear_session_overlay(view.session_id, &file_path)
                                    {
                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                    }
                                }
                            }
                            return Flow::Next;
                        }
                        // A request the in-memory engine has no answer for is refused rather
                        // than left without a reply, which an editor waits on for good.
                        Some(m) if id.as_ref().is_some_and(|i| !i.is_null()) => {
                            let refused = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32601, "message": format!("{m} is not supported by prod-code for Rust") }
                            });
                            let _ = out_tx
                                .send(WireMessage::LspPayload(refused.to_string()))
                                .await;
                            return Flow::Next;
                        }
                        _ => {}
                    }
                }

                // 5. Fallback handling for textDocument/didOpen vs didChange on backend worker
                if let (Some("textDocument/didOpen"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let is_open = backend.open_files.read().await.contains(uri);
                    if is_open {
                        let text = val
                            .get("params")
                            .and_then(|p| p.get("textDocument"))
                            .and_then(|td| td.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let version = val
                            .get("params")
                            .and_then(|p| p.get("textDocument"))
                            .and_then(|td| td.get("version"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(2);
                        let did_change = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/didChange",
                            "params": {
                                "textDocument": {
                                    "uri": uri,
                                    "version": version
                                },
                                "contentChanges": [
                                    { "text": text }
                                ]
                            }
                        });
                        let _ = backend.send_lsp(&did_change.to_string()).await;
                        return Flow::Next;
                    } else {
                        backend.open_files.write().await.insert(uri.to_string());
                    }
                }

                // 5a'. Code actions on managed language servers: the Rust-style
                // prodCode/assists | applyAssist requests become LSP codeAction.
                if let (Some(pm @ ("prodCode/assists" | "prodCode/applyAssist")), Some(req_id)) =
                    (method, &id)
                    && (view.workspace.go_engine.is_some()
                        || view.workspace.generic_engine.is_some())
                {
                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                    let go = view.workspace.go_engine.clone();
                    let generic = view.workspace.generic_engine.clone();
                    let out_tx_task = out_tx.clone();
                    let translator_task = translator.clone();
                    let r_id = req_id.clone();
                    let session_id = view.session_id;
                    let method_name = pm.to_string();
                    let start = Instant::now();
                    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                    tokio::task::spawn(async move {
                        let engine = match (&go, &generic) {
                            (_, Some(g)) => ManagedLsp::Generic(g),
                            (Some(g), None) => ManagedLsp::Go(g),
                            (None, None) => unreachable!("guarded above"),
                        };
                        let outcome = lsp_code_actions(&engine, &method_name, params).await;
                        let ms = start.elapsed().as_secs_f64() * 1000.0;
                        let resp = match outcome {
                            Ok(result) => {
                                tracing::info!(session = session_id, method = %method_name, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": result })
                            }
                            Err(err) => {
                                tracing::info!(session = session_id, method = %method_name, error = %err, "🚫 [LSP REFUSED] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32602, "message": err.to_string() } })
                            }
                        };
                        let client_resp =
                            translator_task.translate_lsp_to_client(&resp.to_string());
                        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                    });
                    return Flow::Next;
                }

                // 5b. Supervised GoEngine fast path
                if let Some(ref go) = view.workspace.go_engine {
                    match method {
                        Some("textDocument/didOpen") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_open(uri, text).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didChange") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let version = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("version"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(1) as i32;
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("contentChanges"))
                                .and_then(|c| c.as_array())
                                .and_then(|a| a.first())
                                .and_then(|ch| ch.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_change(uri, text, version).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didClose") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let _ = go.did_close(uri).await;
                            return Flow::Next;
                        }
                        Some(m) if id.is_some() => {
                            let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                            let start = Instant::now();

                            tracing::info!(
                                req = req_id_log,
                                session = view.session_id,
                                method = m,
                                in_flight,
                                "🚀 [LSP START] dispatching to GoEngine"
                            );

                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
                            let out_tx_task = out_tx.clone();
                            let go_clone = Arc::clone(go);
                            let translator_task = translator.clone();
                            let session_id = view.session_id;
                            let method_str = m.to_string();
                            let req_id = id.clone();

                            tokio::task::spawn(async move {
                                let resp_res = go_clone.send_request(&method_str, params).await;
                                let duration = start.elapsed();
                                let duration_ms = duration.as_secs_f64() * 1000.0;
                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                if duration_ms > 200.0 {
                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                    tracing::warn!(
                                        req = req_id_log,
                                        session = session_id,
                                        method = %method_str,
                                        duration_ms = %format!("{:.2}ms", duration_ms),
                                        in_flight = remaining,
                                        "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                                    );
                                } else {
                                    tracing::info!(
                                        req = req_id_log,
                                        session = session_id,
                                        method = %method_str,
                                        duration_ms = %format!("{:.2}ms", duration_ms),
                                        in_flight = remaining,
                                        "✅ [LSP DONE] GoEngine query complete"
                                    );
                                }

                                match resp_res {
                                    Ok(mut resp) => {
                                        send_busy_note(&mut resp, &out_tx_task).await;
                                        if let Some(ref r_id) = req_id {
                                            resp["id"] = r_id.clone();
                                        }
                                        let client_resp = translator_task
                                            .translate_lsp_to_client(&resp.to_string());
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(client_resp))
                                            .await;
                                    }
                                    Err(err) => {
                                        let err_resp = serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": req_id,
                                            "error": { "code": -32603, "message": err.to_string() }
                                        });
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(err_resp.to_string()))
                                            .await;
                                    }
                                }
                            });
                            return Flow::Next;
                        }
                        Some(m) => {
                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
                            let _ = go.send_notification(m, params).await;
                            return Flow::Next;
                        }
                        None => {}
                    }
                }

                // 5c. Supervised GenericLspEngine fast path
                if let Some(ref generic_eng) = view.workspace.generic_engine {
                    if let (Some("textDocument/rename"), Some(req_id)) = (method, &id) {
                        // Servers such as pyright only rename inside open documents:
                        // open every file that references the symbol first.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        let session_id = view.session_id;
                        let start = Instant::now();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let (resp, opened) = rename_with_references_open(&engine, params).await;
                            tracing::info!(
                                session = session_id,
                                opened,
                                duration_ms =
                                    format!("{:.2}ms", start.elapsed().as_secs_f64() * 1000.0),
                                "✅ [LSP DONE] generic rename"
                            );
                            let resp = match resp {
                                Ok(mut resp) => {
                                    send_busy_note(&mut resp, &out_tx_task).await;
                                    resp["id"] = r_id;
                                    resp
                                }
                                Err(err) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                }
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
                    if let (Some("textDocument/diagnostic"), Some(req_id)) = (method, &id) {
                        // Servers without pull diagnostics answer from what they
                        // published for the document.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let uri = params
                                .get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            let resp = match ManagedLsp::Generic(&engine)
                                .diagnostics_for(&uri)
                                .await
                            {
                                Ok(items) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": { "kind": "full", "items": items } })
                                }
                                // No report is not a clean one (#471).
                                Err(err) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                }
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
                    if let (Some(m), Some(req_id)) = (method, &id) {
                        let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                        let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        let start = Instant::now();

                        tracing::info!(
                            req = req_id_log,
                            session = view.session_id,
                            method = m,
                            in_flight,
                            "🚀 [LSP START] dispatching to GenericLspEngine"
                        );

                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let out_tx_task = out_tx.clone();
                        let generic_eng_clone = Arc::clone(generic_eng);
                        let translator_task = translator.clone();
                        let session_id = view.session_id;
                        let method_str = m.to_string();
                        let r_id = req_id.clone();

                        tokio::task::spawn(async move {
                            let resp_res =
                                generic_eng_clone.send_request(&method_str, params).await;
                            let duration = start.elapsed();
                            let duration_ms = duration.as_secs_f64() * 1000.0;
                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                            if duration_ms > 200.0 {
                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(
                                    req = req_id_log,
                                    session = session_id,
                                    method = %method_str,
                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                    in_flight = remaining,
                                    "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                                );
                            } else {
                                tracing::info!(
                                    req = req_id_log,
                                    session = session_id,
                                    method = %method_str,
                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                    in_flight = remaining,
                                    "✅ [LSP DONE] GenericLspEngine query complete"
                                );
                            }

                            match resp_res {
                                Ok(mut resp) => {
                                    send_busy_note(&mut resp, &out_tx_task).await;
                                    resp["id"] = r_id;
                                    let client_resp =
                                        translator_task.translate_lsp_to_client(&resp.to_string());
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(client_resp))
                                        .await;
                                }
                                Err(err) => {
                                    let err_resp = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": r_id,
                                        "error": { "code": -32603, "message": err.to_string() }
                                    });
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(err_resp.to_string()))
                                        .await;
                                }
                            }
                        });
                        return Flow::Next;
                    } else if let Some(m) = method {
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let _ = generic_eng
                            .send_session_notification(view.session_id, m, params)
                            .await;
                        return Flow::Next;
                    }
                }

                // 6. Handle "textDocument/didClose"
                if let (Some("textDocument/didClose"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    backend.open_files.write().await.remove(uri);
                }

                // 7. If no backend is attached and client expects a response, return empty result
                if let (Some(req_id), None, None, None, None) = (
                    id,
                    &view.workspace.backend,
                    &view.workspace.rust_engine,
                    &view.workspace.go_engine,
                    &view.workspace.generic_engine,
                ) {
                    let empty_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": null
                    });
                    let _ = out_tx
                        .send(WireMessage::LspPayload(empty_resp.to_string()))
                        .await;
                    return Flow::Next;
                }
            }

            if let Some(backend) = &view.workspace.backend {
                let _ = backend.send_lsp(&server_lsp).await.inspect_err(|e| {
                    tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                });
            }
        }
        Some(Ok(WireMessage::SyncRequest(req))) => {
            let start = Instant::now();
            let mut files_updated = 0;
            let mut files_deleted = 0;
            let mut bytes_transferred = 0;
            let mut watched = Vec::new();
            let mut failed: Vec<String> = Vec::new();

            for delta in &req.files {
                let target_path = view.workspace.root.join(&delta.relative_path);
                match &delta.content {
                    Some(content_bytes) => {
                        bytes_transferred += content_bytes.len();
                        let kind = if target_path.exists() {
                            workspace::WatchedChange::Changed
                        } else {
                            workspace::WatchedChange::Created
                        };
                        if let Err(e) =
                            write_synced_file(&target_path, content_bytes, delta.is_executable)
                                .await
                        {
                            tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                            failed.push(delta.relative_path.clone());
                            continue;
                        }
                        files_updated += 1;
                        watched.push((target_path.clone(), kind));
                        // The workspace is this worktree's own: synced files are its
                        // new base, visible to every session except one that still
                        // holds an unsaved buffer for the same path.
                        if let Ok(text) = std::str::from_utf8(content_bytes) {
                            for engine_lock in view.workspace.mirrored_rust_engines() {
                                let mut engine = engine_lock.lock().await;
                                let res =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        engine.update_base(&target_path, Some(text.to_string()))
                                    }));
                                match res {
                                    Ok(Err(e)) => {
                                        tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                    }
                                    Err(_) => {
                                        tracing::warn!(file = %target_path.display(), "base update panicked; continuing");
                                    }
                                    Ok(Ok(())) => {}
                                }
                            }
                        }
                    }
                    None => {
                        if target_path.exists()
                            && tokio::fs::remove_file(&target_path).await.is_ok()
                        {
                            files_deleted += 1;
                            watched.push((target_path.clone(), workspace::WatchedChange::Deleted));
                            prune_empty_parents(&view.workspace.root, target_path.parent());
                        }
                        for engine_lock in view.workspace.mirrored_rust_engines() {
                            let mut engine = engine_lock.lock().await;
                            let res =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    engine.update_base(&target_path, None)
                                }));
                            match res {
                                Ok(Err(e)) => {
                                    tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                                }
                                Err(_) => {
                                    tracing::warn!(file = %target_path.display(), "base removal panicked; continuing");
                                }
                                Ok(Ok(())) => {}
                            }
                        }
                    }
                }
            }

            if req.clean_others
                && let Some(engine_lock) = &view.workspace.rust_engine
            {
                // The request is the session's complete dirty set: any other
                // overlay this session still holds is stale (reverted or committed).
                let keep: Vec<PathBuf> = req
                    .files
                    .iter()
                    .map(|delta| view.workspace.root.join(&delta.relative_path))
                    .collect();
                let mut engine = engine_lock.lock().await;
                match engine.retain_session_overlays(view.session_id, &keep) {
                    Ok(dropped) if dropped > 0 => tracing::info!(
                        session = view.session_id,
                        dropped,
                        "🧹 [OVERLAY] dropped stale session buffers after full dirty sync"
                    ),
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, session = view.session_id, "failed to drop stale session buffers")
                    }
                }
            }

            let mut stale_paths = workspace::clear_stale_paths(
                &view.workspace.root,
                req.files.iter().map(|delta| delta.relative_path.as_str()),
            );
            workspace::record_stale_paths(&view.workspace.root, &failed);
            for path in failed {
                if !stale_paths.contains(&path) {
                    stale_paths.push(path);
                }
            }
            view.workspace.notify_watched_files(&watched).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            let _ = out_tx
                .send(WireMessage::SyncResponse(SyncResponse {
                    files_updated,
                    files_deleted,
                    bytes_transferred,
                    duration_ms,
                    server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                    workspace_was_fresh: false,
                    stale_paths,
                }))
                .await;
        }
        Some(Ok(WireMessage::Disconnect { reason })) => {
            tracing::info!(reason, "Client terminated session");
            return Flow::Stop;
        }
        Some(Ok(WireMessage::StatusRequest)) => {
            let _ = out_tx
                .send(WireMessage::StatusResponse(StatusResponse {
                    server_pid: std::process::id(),
                    uptime_seconds: 0,
                    active_sessions: 1,
                    loaded_workspaces: 1,
                    detected_engines: vec![view.workspace.engine.clone()],
                    memory_rss_bytes: memory::get_process_rss_bytes(),
                    total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
                    active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
                    load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
                    cpu_count: std::thread::available_parallelism().ok().map(|n| n.get()),
                    platform: Some(prod_code_protocol::platform()),
                    running_commands: running_commands(),
                    host: memory::host_resources(&view.worktree_root),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    git_commit: Some(prod_code_protocol::git_commit().to_string())
                        .filter(|c| c != "unknown"),
                }))
                .await;
        }
        Some(Ok(WireMessage::ReadFileRequest(req))) => {
            let resp = read_server_file(&meta.storage_root, &req);
            let _ = out_tx.send(WireMessage::ReadFileResponse(resp)).await;
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "TCP frame decode error");
            return Flow::Stop;
        }
        None => {
            tracing::debug!("Client disconnected");
            return Flow::Stop;
        }
        _ => {}
    }
    Flow::Next
}

#[cfg(test)]
mod tests;
