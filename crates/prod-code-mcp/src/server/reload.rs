/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::hot_reload;
use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::task::{AbortHandle, JoinError, JoinSet};

pub(super) type ToolResponse = (Option<String>, Option<SocketAddr>, Option<String>);
pub(super) type ToolCalls = JoinSet<ToolResponse>;
pub(super) type ActiveCalls = HashMap<String, AbortHandle>;

pub(super) struct Reload {
    pub exe: Option<PathBuf>,
    pub requested: Arc<AtomicBool>,
    pub notify: Arc<Notify>,
    pub timeout: Duration,
    /// How often the loop asks whether the cluster has a better node for this workspace.
    pub rebalance_every: Duration,
    pub reexec: Box<dyn FnMut(&Path) -> std::io::Error + Send>,
}

/// Deliver completed calls before swapping executables. Stop reading stdin while draining:
/// requests still in the pipe belong to the new process, and cannot extend this deadline.
pub(super) async fn drain_for_reload<W: AsyncWrite + Unpin>(
    calls: &mut ToolCalls,
    active: &mut ActiveCalls,
    remote: &mut SocketAddr,
    writer: &mut W,
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    while !calls.is_empty() {
        tokio::select! {
            biased;
            Some(joined) = calls.join_next() => {
                deliver_response(joined, active, remote, writer).await?;
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    if !calls.is_empty() {
        calls.abort_all();
        // Keep results that won the race with cancellation. Do not await aborted handlers:
        // a handler that is slow to observe cancellation must not extend the drain deadline.
        while let Some(joined) = calls.try_join_next() {
            deliver_response(joined, active, remote, writer).await?;
        }
        *calls = JoinSet::new();
    }
    for (id, _) in active.drain() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": serde_json::from_str::<serde_json::Value>(&id)?,
            "error": {
                "code": -32001,
                "message": "Server is reloading; retry this call after the tool list changes",
                "data": { "retryable": true }
            }
        });
        let mut out = super::transport::bound_serialized_response(
            response,
            super::transport::MAX_JSONRPC_FRAME_BYTES,
        );
        out.push('\n');
        writer.write_all(out.as_bytes()).await?;
    }
    writer.flush().await?;
    Ok(())
}

pub(super) async fn deliver_response<W: AsyncWrite + Unpin>(
    joined: std::result::Result<ToolResponse, JoinError>,
    active: &mut ActiveCalls,
    remote: &mut SocketAddr,
    writer: &mut W,
) -> Result<()> {
    if let Ok((id, changed_remote, output)) = joined {
        if let Some(id) = id {
            active.remove(&id);
        }
        if let Some(new_remote) = changed_remote {
            *remote = new_remote;
        }
        if let Some(out) = output {
            writer.write_all(out.as_bytes()).await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

impl Reload {
    pub fn new(exe: Option<PathBuf>) -> Self {
        let requested = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());
        if let Some(exe) = &exe
            && let Some(initial) = hot_reload::stamp(exe)
        {
            hot_reload::spawn_watch(exe.clone(), initial, requested.clone(), notify.clone());
        }
        Self {
            exe,
            requested,
            notify,
            timeout: Duration::from_secs(30),
            rebalance_every: Duration::from_secs(30),
            reexec: Box::new(hot_reload::reexec),
        }
    }
}
