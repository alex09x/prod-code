/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Failover of a pinned workspace (#1793). A call is retried only when its error shows it never
//! started, only on an address the pin allows, and without a cluster search that could find any
//! other node. A strict pin keeps to the other addresses of its one endpoint.

use std::net::SocketAddr;
use std::path::Path;

use crate::cluster::NodePolicy;

/// Whether `e` shows the call never reached a gateway that could have started it: the
/// connection or its pre-flight sync failed, or the node turned the session away for capacity.
/// A reset, a closed connection or a timeout after the request was sent may have run it.
pub fn is_unstarted_connection_error(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_lowercase();
    if msg.contains("result is unknown") || msg.contains("during exec") {
        return false;
    }
    msg.contains("failed to connect to remote gateway")
        || msg.contains("failed to connect to gateway")
        || msg.contains("timed out connecting to gateway")
        || msg.contains("connection refused")
        || msg.contains("no route to host")
        || msg.contains("network is unreachable")
        || msg.contains("gateway refused the session: capacity")
        || msg.contains("refused for capacity")
        || msg.contains("pre-flight workspace sync failed")
}

/// Whether a failed `tool_name` call may be tried again elsewhere under `policy`.
pub fn retry_permitted(policy: &NodePolicy, tool_name: &str, e: &anyhow::Error) -> bool {
    super::failover::is_retryable_connection_error(tool_name, e)
        && (!policy.is_pinned() || is_unstarted_connection_error(e))
}

/// The address a pinned workspace moves to after `old` could not be reached: the first of the
/// pin's other addresses that answers a status request and serves the workspace's engine.
/// No discovery runs and no placement is asked for, so no unlisted node is ever named.
pub async fn rediscover_pinned<S, SF>(
    old: SocketAddr,
    workspace_root: &Path,
    policy: &NodePolicy,
    node_status: S,
) -> Option<SocketAddr>
where
    S: Fn(SocketAddr) -> SF,
    SF: std::future::Future<Output = anyhow::Result<prod_code_protocol::StatusResponse>>,
{
    let (_, engine) = crate::sync::engine_project(workspace_root, workspace_root);
    for candidate in policy.alternatives(old) {
        match node_status(candidate).await {
            Ok(status) if engine.is_none_or(|e| crate::cluster::supports_engine(&status, e)) => {
                tracing::warn!(
                    %old,
                    new = %candidate,
                    policy = %policy.describe(),
                    "pinned workspace moves to another allowed address"
                );
                return Some(candidate);
            }
            _ => continue,
        }
    }
    tracing::warn!(%old, policy = %policy.describe(), "no other allowed address answers");
    None
}

/// The note a retried call carries, so its caller sees where it actually ran.
pub fn relocation_note(old: SocketAddr, new: SocketAddr, e: &anyhow::Error) -> String {
    let first = format!("{e:#}");
    let first = first.lines().next().unwrap_or_default();
    format!(
        "[prod-code] relocated: the gateway at {old} failed the call ({first}); it ran on {new}."
    )
}

#[cfg(test)]
#[path = "failover_pin_tests.rs"]
mod tests;
