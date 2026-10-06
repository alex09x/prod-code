/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::WireMessage;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

use crate::sync::{push_workspace_sync, workspace_identity};

use super::types::{LspSession, budget_for, record_indexing_note, text_hash};

impl LspSession {
    /// Whether the gateway holds this session's index questions until its server is ready
    /// (#391).
    pub fn index_gated(&self) -> bool {
        self.index_gated
    }

    /// How long ago the gateway loaded this session's engine; `None` when it does not say
    /// (#381).
    pub fn engine_age(&self) -> Option<std::time::Duration> {
        self.engine_loaded.map(|at| at.elapsed())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    pub(crate) async fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        Ok(())
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        let budget = budget_for(method);
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("timeout waiting for {method}");
            }
            match tokio::time::timeout(remaining, self.framed.next()).await {
                Ok(Some(Ok(WireMessage::LspPayload(json)))) => {
                    let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    // The server was still loading or indexing when the gateway asked it: the
                    // answer that follows may be incomplete, and the tool says so (#391).
                    if val.get("method").and_then(|m| m.as_str())
                        == Some(prod_code_protocol::readiness::BUSY_NOTIFICATION)
                        && let Some(busy) = val.get("params").and_then(|p| {
                            serde_json::from_value::<prod_code_protocol::readiness::Busy>(p.clone())
                                .ok()
                        })
                    {
                        record_indexing_note(&self.root, busy.describe());
                        continue;
                    }
                    // An answer has no method: a request from the server that carries the same
                    // id is not it (#391).
                    if val.get("method").is_some()
                        || val.get("id").and_then(|i| i.as_i64()) != Some(id)
                    {
                        continue;
                    }
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
                    return val
                        .get("result")
                        .cloned()
                        .with_context(|| format!("{method} response has no result"));
                }
                Ok(Some(Ok(WireMessage::Ping))) => {
                    let _ = self.framed.send(WireMessage::Pong).await;
                }
                Ok(Some(Ok(WireMessage::Redirect {
                    target_addr,
                    reason,
                }))) => {
                    tracing::info!(%target_addr, ?reason, "received dynamic redirect mid-session from gateway");
                    let ws_identity = workspace_identity(&self.root);
                    if let Some(addr) = crate::cluster::parse_remotes(&target_addr)
                        .ok()
                        .and_then(|addrs| addrs.into_iter().next())
                    {
                        crate::cluster::remember_placement(&ws_identity.name, addr);
                    }
                    anyhow::bail!(
                        "session rebalanced to {target_addr}: {}",
                        reason.unwrap_or_default()
                    );
                }
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => anyhow::bail!("frame decode error: {e}"),
                Ok(None) => anyhow::bail!("gateway closed the session"),
                Err(_) => anyhow::bail!("timeout waiting for {method}"),
            }
        }
    }

    /// Opens `file` (once) and runs `method` with `params` on it.
    pub async fn query(
        &mut self,
        file: &Path,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        // A directory stands for a workspace-level query (workspace/symbol): nothing to open.
        // Neither does a file outside the checkout that is not on this machine, such as a
        // dependency's source in the node's cargo registry: the node's analyzer already has
        // it, and reading it here would fail (#271).
        let only_on_the_node = !abs.starts_with(&self.root) && !abs.exists();
        if !self.opened.contains_key(&uri) && !abs.is_dir() && !only_on_the_node {
            let text = tokio::fs::read_to_string(&abs)
                .await
                .with_context(|| format!("failed to read {}", abs.display()))?;
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened
                .insert(uri.clone(), (abs.clone(), Some(text_hash(&text))));
        }
        self.request(method, params).await
    }

    /// Runs `method` on `file` with `text` as its content instead of what is on disk: the
    /// document is opened (or changed) with the proposed text, so the server analyses the
    /// edit without anything being written.
    pub async fn query_with_text(
        &mut self,
        file: &Path,
        text: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.open_text(file, text).await?;
        self.request(method, params).await
    }

    /// Makes `text` the content of `file` in this session (didOpen, or didChange when the
    /// document is already open) without querying, so several proposed files can be in
    /// place before diagnostics are pulled for any of them. Returns the document's URI.
    pub async fn open_text(&mut self, file: &Path, text: &str) -> Result<String> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        if self.opened.contains_key(&uri) {
            self.opened.insert(uri.clone(), (abs.clone(), None));
            self.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": uri, "version": self.next_id },
                    "contentChanges": [ { "text": text } ]
                }),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened.insert(uri.clone(), (abs.clone(), None));
        }
        Ok(uri)
    }

    /// The `file://` URI the session uses for `file`.
    pub fn uri_for(&self, file: &Path) -> Result<String> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        Ok(Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string())
    }

    /// Pushes the checkout's changes since the last sync over this same connection and
    /// brings the documents the session holds open in line with the files (`didChange` for a
    /// rewritten file, `didClose` for a deleted one), so a long-lived session sees every local
    /// edit exactly like a fresh one would. That is every file this sync pushed, and every file
    /// opened from disk whose text is no longer what was sent: another client (the CLI, an
    /// `exec` whose formatter's output came back, another agent) may have pushed it to the node
    /// already, and this sync then pushes nothing for it (#360).
    pub async fn refresh(&mut self) -> Result<()> {
        crate::call_tree::clear_call_hierarchy_cache_for(&self.root);
        let identity = workspace_identity(&self.root);
        let outcome = push_workspace_sync(&mut self.framed, &self.root, &identity, None)
            .await
            .context("workspace sync on the open session failed")?;
        let pushed: HashSet<String> = outcome
            .changed_paths
            .iter()
            .filter_map(|rel| Url::from_file_path(self.root.join(rel)).ok())
            .map(|uri| uri.to_string())
            .collect();
        let open: Vec<(String, PathBuf, Option<u64>)> = self
            .opened
            .iter()
            .map(|(uri, (path, sent))| (uri.clone(), path.clone(), *sent))
            .collect();
        for (uri, path, sent) in open {
            let was_pushed = pushed.contains(&uri);
            if !was_pushed && sent.is_none() {
                continue;
            }
            match tokio::fs::read_to_string(&path).await {
                Ok(text) => {
                    let hash = text_hash(&text);
                    if !was_pushed && sent == Some(hash) {
                        continue;
                    }
                    let version = self.next_id;
                    self.next_id += 1;
                    self.notify(
                        "textDocument/didChange",
                        serde_json::json!({
                            "textDocument": { "uri": uri, "version": version },
                            "contentChanges": [ { "text": text } ]
                        }),
                    )
                    .await?;
                    self.opened.insert(uri, (path, Some(hash)));
                }
                Err(_) => {
                    self.notify(
                        "textDocument/didClose",
                        serde_json::json!({ "textDocument": { "uri": uri } }),
                    )
                    .await?;
                    self.opened.remove(&uri);
                }
            }
        }
        Ok(())
    }

    /// Ends the session cleanly.
    pub async fn close(mut self) {
        for uri in std::mem::take(&mut self.opened).into_keys() {
            let _ = self
                .notify(
                    "textDocument/didClose",
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                )
                .await;
        }
        let _ = self
            .framed
            .send(WireMessage::Disconnect {
                reason: "batch finished".to_string(),
            })
            .await;
    }
}
