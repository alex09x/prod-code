/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One `tools/call`, and its single retry on another node, under the deadline set when the
//! request was read (#1814). The retry spends what is left of the first attempt's budget; when
//! nothing is left, the first error is the answer and the workspace stays where it was.

use anyhow::Result;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::time::Instant;

use crate::cluster::NodePolicy;
use crate::protocol::{McpContentItem, McpToolCallResult};
use crate::server::failover_pin::{relocation_note, retry_permitted};
use crate::tools::{execute_tool_at, navigation_deadline};

/// Runs `tool_name` on `remote`; if its gateway could not be reached and `policy` permits,
/// asks `rediscover` for another node and runs it there once, all before the navigation
/// deadline counted from `received`. A call without such a deadline is bounded by its caller.
pub(crate) async fn run_tool_call<D, F>(
    remote: &mut SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    mut arguments: serde_json::Value,
    policy: &NodePolicy,
    received: Instant,
    rediscover: D,
) -> Result<McpToolCallResult>
where
    D: Fn(SocketAddr, PathBuf) -> F,
    F: std::future::Future<Output = Option<SocketAddr>>,
{
    let selector = if tool_name == "code_workflow" {
        "code_definition"
    } else {
        tool_name
    };
    let selected = crate::tools::resolve_workspace_root(workspace_root, selector, &mut arguments)?;
    let selected_policy = crate::cluster::pin_scope::effective_policy(&selected, policy)?;
    let home =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let is_home = selected == home;
    let mut call_remote = crate::cluster::pin_scope::place_within(&selected_policy, *remote)?;
    let outcome = crate::cluster::with_root_policy(
        &selected,
        selected_policy.clone(),
        call_under_policy(
            &mut call_remote,
            &selected,
            tool_name,
            arguments,
            &selected_policy,
            received,
            rediscover,
            is_home,
        ),
    )
    .await;
    if is_home {
        *remote = call_remote;
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn call_under_policy<D, F>(
    remote: &mut SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    arguments: serde_json::Value,
    policy: &NodePolicy,
    received: Instant,
    rediscover: D,
    is_home: bool,
) -> Result<McpToolCallResult>
where
    D: Fn(SocketAddr, PathBuf) -> F,
    F: std::future::Future<Output = Option<SocketAddr>>,
{
    let deadline = navigation_deadline(tool_name, &arguments, received);
    let first = execute_tool_at(
        *remote,
        workspace_root,
        tool_name,
        arguments.clone(),
        received,
    )
    .await;
    let e = match first {
        Ok(call_result) => return Ok(call_result),
        Err(e)
            if retry_permitted(policy, tool_name, &e)
                && (!policy.is_pinned() || !policy.alternatives(*remote).is_empty()) =>
        {
            e
        }
        Err(e) => return Err(e),
    };
    // Gateway unreachable before dispatch — try UDP discovery to find a live node.
    tracing::warn!(
        old_gateway = %remote,
        error = %e,
        tool = tool_name,
        "gateway unreachable, running UDP discovery"
    );
    let old = *remote;
    let search = rediscover(old, workspace_root.to_path_buf());
    let found = match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, search)
            .await
            .ok()
            .flatten(),
        None => search.await,
    };
    let Some(new_addr) = found.filter(|addr| policy.allows(*addr)) else {
        return Err(e);
    };
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        tracing::warn!(old = %old, new = %new_addr, "no budget left to retry on another node");
        return Err(e);
    }
    *remote = new_addr;
    if is_home && !policy.is_pinned() {
        let identity = crate::sync::workspace_identity(workspace_root);
        let name = identity.base.unwrap_or(identity.name);
        crate::cluster::remember_placement(&name, new_addr);
    }
    tracing::info!(
        old = %old,
        new = %new_addr,
        "re-placed to a live gateway via UDP discovery"
    );
    // Retry the tool call on the new node with the same deadline, and say that it moved.
    let note = relocation_note(old, new_addr, &e);
    execute_tool_at(*remote, workspace_root, tool_name, arguments, received)
        .await
        .map(|mut result| {
            result.content.push(McpContentItem::Text { text: note });
            result
        })
}
