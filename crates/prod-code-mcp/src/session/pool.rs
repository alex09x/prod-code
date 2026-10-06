/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::sync::{engine_project, workspace_identity};

use super::types::{LspSession, OPEN_BUDGET, budget_for, timeout_error};

/// Long-lived sessions of this process, one per (gateway, checkout, nested project): the
/// MCP server keeps them across tool calls so a query costs one round trip instead of a
/// connection, a sync and a handshake each time.
pub(crate) type SessionSlot = Arc<tokio::sync::Mutex<Option<LspSession>>>;

pub(crate) fn pool() -> &'static tokio::sync::Mutex<HashMap<String, SessionSlot>> {
    static POOL: OnceLock<tokio::sync::Mutex<HashMap<String, SessionSlot>>> = OnceLock::new();
    POOL.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

pub(crate) async fn session_slot(key: &str) -> SessionSlot {
    let mut sessions = pool().lock().await;
    Arc::clone(sessions.entry(key.to_string()).or_default())
}

pub(crate) fn is_connection_error(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    text.contains("closed")
        || text.contains("timeout")
        || text.contains("timed out")
        || text.contains("broken pipe")
        || text.contains("reset")
        || text.contains("decode")
        || text.contains("connection")
        || text.contains("rebalanced to")
        // The gateway's language server crashed: a new session gets a new one (#355).
        || text.contains("has exited")
}

/// The checkout a query about `file` is asked in, and the key of its pooled session: one per
/// node, checkout and nested project.
pub(crate) fn pool_key(remote: SocketAddr, root: &Path, file: &Path) -> (PathBuf, String) {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // A local file of another checkout, such as a clone next to this one, is asked about in
    // that checkout's own session: this one's analyzer never loaded it, and a server of another
    // language only fails on it (#353).
    let root = crate::sync::other_checkout(&root, file).unwrap_or(root);
    let (subpath, mut engine) = engine_project(&root, file);
    if let Some(own) = crate::sync::engine_for_file(file)
        && engine == crate::sync::expected_engine(&root)
        && Some(own) != engine
    {
        engine = Some(own);
    }
    let key = format!(
        "{remote}|{}|{}|{}",
        root.display(),
        subpath.unwrap_or_default(),
        engine.unwrap_or_default()
    );
    (root, key)
}

/// [`LspSession::engine_age`] of the pooled session that answers about `file` in the checkout
/// at `root`; `None` when there is none yet or its gateway does not say (#381).
pub async fn pooled_engine_age(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Option<std::time::Duration> {
    let (_, key) = pool_key(remote, root, file);
    let slot = pool().lock().await.get(&key).cloned()?;
    // Metadata is advisory; it must not queue behind a query just to report its age.
    let session = slot.try_lock().ok()?;
    session.as_ref().and_then(LspSession::engine_age)
}

/// [`LspSession::index_gated`] of the pooled session that answers about `file` in the checkout
/// at `root`; `false` when there is none yet (#391).
pub async fn pooled_index_gated(remote: SocketAddr, root: &Path, file: &Path) -> bool {
    let (_, key) = pool_key(remote, root, file);
    let Some(slot) = pool().lock().await.get(&key).cloned() else {
        return false;
    };
    slot.try_lock()
        .ok()
        .and_then(|session| session.as_ref().map(LspSession::index_gated))
        .unwrap_or(false)
}

/// Runs one query on the pooled session for `root` (opening it on first use): local
/// changes are pushed first when the watcher saw any, and a session whose connection died
/// (gateway restart) is replaced and the query retried once.
pub async fn pooled_query(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    pooled_query_with_budget(
        remote,
        root,
        file,
        method,
        params,
        budget_for(method).max(OPEN_BUDGET),
    )
    .await
}

pub(crate) async fn pooled_query_with_budget(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    method: &str,
    params: serde_json::Value,
    budget: std::time::Duration,
) -> Result<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + budget;
    let (root, key) = pool_key(remote, root, file);
    let slot = session_slot(&key).await;
    let mut stored = tokio::time::timeout_at(deadline, slot.lock())
        .await
        .map_err(|_| timeout_error("waiting for another query in this workspace", budget))?;
    // The slot stays empty while the request owns the connection. Cancellation drops the
    // connection too, rather than caching an interrupted sync or an unread reply (#430).
    let mut session = stored.take();
    let result = tokio::time::timeout_at(deadline, async {
        for attempt in 0..2 {
            let mut target_remote = remote;
            let ws_identity = workspace_identity(&root);
            if let Some(remembered) = crate::cluster::remembered_node(&ws_identity.name) {
                target_remote = remembered;
            }
            if let Some(ref current) = session {
                if current.remote != target_remote {
                    tracing::info!(
                        from = %current.remote,
                        to = %target_remote,
                        "retiring established session because placement was rebalanced"
                    );
                    let old_session = session.take().expect("session present");
                    tokio::spawn(async move {
                        old_session.close().await;
                    });
                }
            }
            if session.is_none() {
                session = Some(LspSession::open(target_remote, &root, Some(file)).await?);
                crate::watch::mark_synced(&root, crate::watch::current_generation(&root));
            }
            let current = session.as_mut().expect("just inserted");
            let generation = crate::watch::current_generation(&root);
            let result = async {
                if crate::watch::sync_due(&root, generation) {
                    current.refresh().await?;
                    crate::watch::mark_synced(&root, generation);
                    crate::call_tree::clear_call_hierarchy_cache_for(&root);
                }
                current.query(file, method, params.clone()).await
            }
            .await;
            match result {
                Ok(value) => return Ok(value),
                Err(err) if is_connection_error(&err) => {
                    session = None;
                    // A timeout is already a spent budget; repeating the same analysis
                    // immediately used to double the wait. The next call may retry it.
                    let timeout = format!("{err:#}").to_ascii_lowercase().contains("timeout");
                    if attempt == 0 && !timeout && method != "workspace/executeCommand" {
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("two attempts always return")
    })
    .await;
    match result {
        Ok(result) => {
            *stored = session;
            result
        }
        Err(_) => Err(timeout_error(
            &format!("running {method} (including sync and engine load)"),
            budget,
        )),
    }
}
