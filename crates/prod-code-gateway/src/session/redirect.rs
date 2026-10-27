/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;
use futures_util::SinkExt;
use prod_code_protocol::HandshakeRequest;
use std::path::Path;
use std::sync::Arc;

pub async fn check_cluster_redirects(
    req: &HandshakeRequest,
    state: &Arc<ServerState>,
    engine: &str,
    server_workspace: &Path,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
) -> Result<Option<()>> {
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
                        reason: Some(format!("engine {engine} is served by {}", target.addr)),
                    })
                    .await;
                return Ok(Some(()));
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
        return Ok(Some(()));
    }

    // Roadmap 5.1: If this workspace is not already loaded locally, but another live cluster
    // node has it loaded warm, transparently redirect the client there.
    let is_loaded_locally = state
        .workspace_manager
        .get_loaded(server_workspace)
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
                    && n.status.host.pressure().is_none()
                    && n.status.congestion_score() < 1.0
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
                return Ok(Some(()));
            }
        }

        // Roadmap 5.3: Dynamic workload rebalancing and resource pressure load shedding.
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
                return Ok(Some(()));
            }
        }
    }

    Ok(None)
}

pub async fn try_validation_redirect(
    req: &HandshakeRequest,
    state: &Arc<ServerState>,
    engine: &str,
    session_view: SessionView,
    session_id: u64,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    err: &anyhow::Error,
) -> Result<Option<()>> {
    let Some(target) = validation_redirect_target(req, state, engine, err).await else {
        return Ok(None);
    };
    tracing::info!(
        session_id,
        engine,
        target = %target,
        "redirecting validation session under gateway memory pressure"
    );
    state
        .workspace_manager
        .unregister_session_view(session_view)
        .await;
    let _ = framed
        .send(WireMessage::Redirect {
            reason: Some(format!(
                "gateway memory pressure; validating {engine} on {target}"
            )),
            target_addr: target,
        })
        .await;
    Ok(Some(()))
}

/// The node a validation handshake refused for capacity (`err`) is sent to, if any. Only a
/// client that said it follows redirects gets one: an older client cannot read the frame, and
/// is refused with the reason instead (#1495).
pub(crate) async fn validation_redirect_target(
    req: &HandshakeRequest,
    state: &Arc<ServerState>,
    engine: &str,
    err: &anyhow::Error,
) -> Option<String> {
    let supports_redirects = req
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.redirects);
    let is_capacity = err
        .downcast_ref::<crate::admission::CapacityRefused>()
        .is_some()
        || err.root_cause().is::<crate::admission::CapacityRefused>()
        || err.to_string().contains("capacity:");
    if !supports_redirects || !is_capacity || req.redirect_count >= 2 {
        return None;
    }
    let view = state.cluster_view().await;
    let own_addr = state.advertise.read().await.clone();
    let ws_name = req
        .base_workspace_name
        .as_deref()
        .unwrap_or(&req.client_workspace_root);
    let reserve = state.workspace_manager.admission().reserve_for(engine);
    view.nodes
        .iter()
        .filter(|n| {
            n.alive
                && !n.addr.is_empty()
                && n.addr != own_addr
                && n.addr != view.this_node
                && cluster_supports_engine(&n.status, engine)
                && n.status.host.pressure().is_none()
                && (req.redirect_count == 0 || !n.workspaces.iter().any(|w| w.name == ws_name))
        })
        .filter_map(|n| {
            // A peer without the workspace warm loads its main engine before the validation
            // one, so it needs room for both (#1496).
            let warm = n.workspaces.iter().any(|w| w.name == ws_name);
            let needed = if warm {
                reserve
            } else {
                reserve.saturating_mul(2)
            };
            let host = &n.status.host;
            let spare = match (host.memory_available_bytes, host.memory_total_bytes) {
                // What the peer would admit, as `Admission::try_reserve` counts it: its limit
                // less what is in use. The free memory it gossips has its own reservations
                // taken off already, so a big, nearly full host ranks below a small idle one.
                (Some(available), Some(total)) if total > 0 => {
                    let used = total.saturating_sub(available.min(total));
                    crate::admission::types::limit_bytes(total)
                        .saturating_sub(used)
                        .checked_sub(needed)?
                }
                (Some(available), _) => available.checked_sub(needed)?,
                // A host whose memory is not known admits every engine, as placement does,
                // and ranks below every host with known room.
                (None, _) => 0,
            };
            Some((spare, n))
        })
        .max_by_key(|(spare, _)| *spare)
        .map(|(_, n)| n.addr.clone())
}

/// What a session told to move to `target_addr` sends its client: a redirect it follows, or,
/// for a client that never said it reads one, a disconnect naming where to reconnect (#1495).
pub(crate) fn rebalance_frame(
    redirects: bool,
    target_addr: String,
    reason: Option<String>,
) -> WireMessage {
    if redirects {
        return WireMessage::Redirect {
            target_addr,
            reason,
        };
    }
    let why = reason.map(|r| format!(" ({r})")).unwrap_or_default();
    WireMessage::Disconnect {
        reason: format!("this workspace moved to {target_addr}{why}; reconnect there"),
    }
}

#[cfg(test)]
mod tests;
