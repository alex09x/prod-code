/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::handshake::handle_handshake;
use crate::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;

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

    let addr_str = addr.to_string();
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
                let start = std::time::Instant::now();
                let status = state.status().await;
                let mut ev = metrics::Event::blank("status");
                ev.method = "status".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                state.metrics.record(ev);
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::Gossip(gossip) => {
                state.absorb_gossip(gossip).await;
                let own = state.own_gossip().await;
                framed.send(WireMessage::Gossip(own)).await?;
            }
            WireMessage::ClusterRequest => {
                let start = std::time::Instant::now();
                let view = state.cluster_view().await;
                let mut ev = metrics::Event::blank("cluster");
                ev.method = "cluster_view".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.items = view.nodes.len() as u64;
                state.metrics.record(ev);
                framed.send(WireMessage::ClusterResponse(view)).await?;
            }
            WireMessage::PlaceRequest(req) => {
                let start = std::time::Instant::now();
                let resp = state.place(&req).await;
                let mut ev = metrics::Event::blank("place");
                ev.method = "place".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.ok = resp.node.is_some();
                state.metrics.record(ev);
                framed.send(WireMessage::PlaceResponse(resp)).await?;
            }
            WireMessage::PlacePreviewRequest(req) => {
                let start = std::time::Instant::now();
                let resp = state.preview_place(&req).await;
                let mut ev = metrics::Event::blank("place");
                ev.method = "place".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.ok = resp.node.is_some();
                state.metrics.record(ev);
                framed.send(WireMessage::PlacePreviewResponse(resp)).await?;
            }
            WireMessage::MetricsRequest(req) => {
                let start = std::time::Instant::now();
                let node = state.advertise.read().await.clone();
                let resp = state.metrics.summary(&node, req.since_secs);
                let mut ev = metrics::Event::blank("metrics");
                ev.method = "metrics_summary".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                state.metrics.record(ev);
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
                let start = std::time::Instant::now();
                let resp =
                    apply_sync_probe(&state.storage_root, &state.workspace_manager, req).await;
                let mut ev = metrics::Event::blank("sync");
                ev.method = "sync_probe".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.items = resp.missing.len() as u64 + resp.files_deleted as u64;
                state.metrics.record(ev);
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
                let start = std::time::Instant::now();
                let resp = {
                    let state = Arc::clone(&state);
                    tokio::task::spawn_blocking(move || {
                        search::run_search(&state.search_indexes, &state.storage_root, &req)
                    })
                    .await?
                };
                let mut ev = metrics::Event::blank("search");
                ev.method = "search".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.items = resp.hits.len() as u64;
                ev.ok = resp.error.is_none();
                if let Some(ref err) = resp.error {
                    ev.error_class = Some(metrics::classify_error(err, None).to_string());
                }
                state.metrics.record(ev);
                framed.send(WireMessage::SearchResponse(resp)).await?;
            }
            WireMessage::ReadFileRequest(req) => {
                let start = std::time::Instant::now();
                let resp = read_server_file(&state.storage_root, &req);
                let mut ev = metrics::Event::blank("read_file");
                ev.method = "read_file".to_string();
                ev.duration_ms = start.elapsed().as_millis() as u64;
                ev.ok = resp.error.is_none();
                ev.bytes = resp.content.as_ref().map(|c| c.len() as u64).unwrap_or(0);
                if let Some(ref err) = resp.error {
                    ev.error_class = Some(metrics::classify_error(err, None).to_string());
                }
                state.metrics.record(ev);
                framed.send(WireMessage::ReadFileResponse(resp)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                return handle_handshake(req, &state, framed, &addr_str).await;
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
