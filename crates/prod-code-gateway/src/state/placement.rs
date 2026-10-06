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

impl ServerState {
    /// Where `workspace_name` should live: the node that already holds it (this one first),
    /// otherwise the quietest live node that serves `engine`; a node loaded but idle on an
    /// overloaded gateway moves to a much quieter one.
    pub async fn place(&self, req: &PlaceRequest) -> PlaceResponse {
        let view = self.cluster_view().await;
        let own_addr = self.advertise.read().await.clone();
        let resp = place_in(req, view);
        if req.rebalance_active
            && let Some(ref target) = resp.node
            && target != &own_addr
        {
            let notified = self
                .workspace_manager
                .trigger_rebalance_by_name(
                    &req.workspace_name,
                    target.clone(),
                    Some(resp.reason.clone()),
                )
                .await;
            if notified > 0 {
                tracing::info!(
                    workspace = %req.workspace_name,
                    target = %target,
                    notified,
                    "triggered dynamic rebalance redirect for active sessions"
                );
            }
        }
        resp
    }

}

/// The node `req` should be placed on, given the cluster `view`: the one that already holds it,
/// otherwise the quietest live node that can serve it and is not short of memory or disk. A
/// holder short of either gives up an idle workspace the way an overloaded one does. A macOS
/// node takes only work that needs macOS, or that no other live node can serve (#308): it is a
/// developer's Mac, running Swift and Go with macOS-only cgo, and plain Go or Rust is placed on
/// the Linux nodes even when the Mac is quieter.
pub(crate) fn place_in(req: &PlaceRequest, view: ClusterResponse) -> PlaceResponse {
    let engine = req.engine.as_deref();
    // A node started with `--engines swift` advertises one engine and serves nothing
    // else. A workspace whose engine the client could not determine must not be sent
    // there: it would be refused at the handshake, or worse, accepted by an older
    // gateway that does not know it is specialised.
    // A node that reports no platform is an older gateway: it cannot be shown to run the
    // OS the workspace needs, so it does not get it.
    let runs_os = |n: &PeerInfo| {
        req.os.as_deref().is_none_or(|os| {
            n.status
                .platform
                .as_deref()
                .is_some_and(|p| p.starts_with(os))
        })
    };
    let capable = |n: &PeerInfo| {
        runs_os(n)
            && match engine {
                Some(e) => cluster_supports_engine(&n.status, e),
                None => n.status.detected_engines.len() > 1,
            }
    };
    let on_macos = |n: &PeerInfo| {
        n.status
            .platform
            .as_deref()
            .is_some_and(|p| p.starts_with("macos"))
    };
    let other_than_macos = req.os.is_none()
        && view
            .nodes
            .iter()
            .any(|n| n.alive && capable(n) && !on_macos(n));
    let capable = |n: &PeerInfo| capable(n) && !(other_than_macos && on_macos(n));
    // A node short of memory or disk takes no new workspace while a capable one that is not can
    // (#396): a full disk truncates the files synced to it (#385), and an engine loaded into a
    // host out of memory pushes it into swap.
    let pressure = |n: &PeerInfo| n.status.host.pressure();
    let roomy_exists = view
        .nodes
        .iter()
        .any(|n| n.alive && capable(n) && pressure(n).is_none());
    let takes_new = |n: &PeerInfo| !(roomy_exists && pressure(n).is_some());
    let score = |n: &PeerInfo| n.status.congestion_score();
    let quietest = view
        .nodes
        .iter()
        .filter(|n| n.alive && capable(n) && takes_new(n))
        .min_by(|a, b| {
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let holder = view.nodes.iter().find(|n| {
        n.alive && capable(n) && n.workspaces.iter().any(|w| w.name == req.workspace_name)
    });
    if let Some(h) = holder {
        let (idle, sessions) = h
            .workspaces
            .iter()
            .find(|w| w.name == req.workspace_name)
            .map(|w| (w.sessions == 0, w.sessions))
            .unwrap_or((true, 0));
        let can_move = idle || req.rebalance_active;
        if let Some(q) = quietest
            && can_move
            && q.addr != h.addr
        {
            let state_str = if idle {
                "idle".to_string()
            } else {
                format!("active, {sessions} sessions")
            };
            if let Some(why) = pressure(h)
                && roomy_exists
            {
                return PlaceResponse {
                    node: Some(q.addr.clone()),
                    reason: format!(
                        "moved from {} ({why}, {state_str}) to {} (score {:.2}, load {:.2}/cpu)",
                        h.addr,
                        q.addr,
                        score(q),
                        q.status.load_per_cpu().unwrap_or(0.0)
                    ),
                };
            }
            if score(h) >= 0.80 && score(q) < score(h) * 0.50 && (score(h) - score(q)) >= 0.40 {
                return PlaceResponse {
                    node: Some(q.addr.clone()),
                    reason: format!(
                        "rebalanced from {} (score {:.2}, {state_str}) to the quieter {} (score {:.2})",
                        h.addr,
                        score(h),
                        q.addr,
                        score(q)
                    ),
                };
            }
        }
        return PlaceResponse {
            node: Some(h.addr.clone()),
            reason: format!("already loaded on {}", h.addr),
        };
    }
    match quietest {
        Some(q) => {
            let mut reason = format!(
                "quietest node serving {} (score {:.2}, load {:.2}/cpu)",
                engine.unwrap_or("any engine"),
                score(q),
                q.status.load_per_cpu().unwrap_or(0.0)
            );
            let passed_over: Vec<String> = view
                .nodes
                .iter()
                .filter(|n| n.alive && capable(n) && !takes_new(n))
                .filter_map(|n| pressure(n).map(|why| format!("{} ({why})", n.addr)))
                .collect();
            if !passed_over.is_empty() {
                reason.push_str(&format!("; passed over {}", passed_over.join(", ")));
            }
            if let Some(why) = pressure(q) {
                reason.push_str(&format!(
                    "; it is short too ({why}), as is every node serving it"
                ));
            }
            PlaceResponse {
                node: Some(q.addr.clone()),
                reason,
            }
        }
        None => PlaceResponse {
            node: None,
            reason: match req.os.as_deref() {
                Some(os) => format!(
                    "no live node runs {os} and serves {}",
                    engine.unwrap_or("this workspace")
                ),
                None => {
                    format!("no live node serves {}", engine.unwrap_or("this workspace"))
                }
            },
        },
    }
}
