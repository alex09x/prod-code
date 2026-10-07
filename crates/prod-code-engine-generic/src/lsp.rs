/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use prod_code_protocol::readiness::{BUSY_MEMBER, Busy, needs_index};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::engine::GenericLspEngine;
use crate::types::{
    DocumentOwner, InitializationGuard, OrdinaryActivity, PendingRequest, lock_unpoisoned,
    validate_initialize_response,
};
use std::path::Path;
use url::Url;

pub(crate) fn workspace_file_uri(path: &Path) -> Result<String> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Url::from_directory_path(&absolute_path)
        .map(|uri| uri.to_string())
        .map_err(|_| anyhow::anyhow!("invalid workspace directory path"))
}

impl GenericLspEngine {
    /// Perform the standard LSP initialize handshake.
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        let mut handshake = InitializationGuard {
            engine: self,
            complete: false,
        };
        *self.capabilities.write().await = None;
        let ws_uri = workspace_file_uri(&self.workspace_root)?;
        let ws_name = self
            .workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("generic-workspace");

        let mut init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": ws_uri.clone(),
            "workspaceFolders": [
                {
                    "name": ws_name,
                    "uri": ws_uri
                }
            ],
            "capabilities": {
                // Servers report loading and indexing as progress only to a client that says
                // it takes it (#391).
                "window": {
                    "workDoneProgress": true
                },
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                },
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "willSaveWaitUntil": false,
                        "didSave": true
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true
                    },
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    "definition": {
                        "linkSupport": true
                    },
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "references": {}
                }
            }
        });

        if let Some(options) = &self.config.initialization_options {
            init_params["initializationOptions"] = options.clone();
        }
        let init_timeout = self.config.request_timeout.max(Duration::from_secs(180));
        let resp = self
            .send_request_with_timeout("initialize", init_params, init_timeout)
            .await?;

        let capabilities = validate_initialize_response(&resp)?;

        self.send_notification("initialized", serde_json::json!({}))
            .await?;
        *self.capabilities.write().await = Some(capabilities);
        self.readiness.started();
        handshake.complete = true;

        Ok(resp)
    }

    /// Whether the server's readiness is known, so that its index answers are final once it
    /// has said it is ready (#391).
    pub fn readiness_known(&self) -> bool {
        self.readiness.known()
    }

    /// The loading or indexing the server is still doing, if any.
    pub fn busy(&self) -> Option<Busy> {
        self.readiness.busy()
    }

    /// Send a request and await its response.
    /// Runs `workspace/executeCommand` and returns the WorkspaceEdit the server pushes back
    /// through `workspace/applyEdit` while doing so (clangd and others deliver refactorings
    /// this way), or `None` when the command finished without an edit.
    pub async fn execute_command_capturing_edit(
        &self,
        command: serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        let (tx, rx) = oneshot::channel();
        *self.apply_edit_waiter.lock().await = Some(tx);
        let params = serde_json::json!({
            "command": command.get("command").cloned().unwrap_or(serde_json::Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(serde_json::json!([])),
        });
        let response = self.send_request("workspace/executeCommand", params).await;
        let edit = match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(edit)) if !edit.is_null() => Some(edit),
            _ => None,
        };
        *self.apply_edit_waiter.lock().await = None;
        let response = response?;
        if let Some(err) = response.get("error") {
            anyhow::bail!(
                "{}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("command failed")
            );
        }
        Ok(edit)
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.send_request_with_timeout(method, params, self.config.request_timeout)
            .await
    }

    pub async fn send_request_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        if !self.is_alive.load(Ordering::Acquire) {
            let details = self
                .exit_details()
                .await
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            anyhow::bail!("Language server process has exited before request '{method}'{details}");
        }
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        // A question answered from the index waits until the server has built it; one still
        // not built when the wait ends is answered with a note of how far it got (#391).
        let busy = if needs_index(method) {
            self.readiness.wait(self.config.index_wait).await
        } else {
            None
        };

        let deadline = tokio::time::Instant::now() + timeout;
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params
        });

        let body = payload.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut writer = tokio::time::timeout_at(deadline, self.stdin.lock())
            .await
            .with_context(|| format!("Timeout waiting to send LSP request '{method}'"))?;
        if !self.is_alive.load(Ordering::Acquire) {
            let details = self
                .exit_details()
                .await
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            anyhow::bail!("Language server process has exited before request '{method}'{details}");
        }
        let (tx, rx) = oneshot::channel();
        lock_unpoisoned(&self.pending_requests).insert(req_id, tx);
        // Declared after `writer`: cancellation or a write error retires the partial stream
        // while this request still owns the writer lock, before a queued writer can wake.
        let mut pending = PendingRequest {
            id: req_id,
            pending: Arc::clone(&self.pending_requests),
            child: Arc::clone(&self._child),
            is_alive: Arc::clone(&self.is_alive),
            frame_written: false,
        };
        tokio::time::timeout_at(deadline, writer.write_all(frame.as_bytes()))
            .await
            .with_context(|| format!("Timeout writing LSP request '{method}'"))?
            .with_context(|| format!("Failed to write LSP request '{method}'"))?;
        tokio::time::timeout_at(deadline, writer.flush())
            .await
            .with_context(|| format!("Timeout flushing LSP request '{method}'"))?
            .with_context(|| format!("Failed to flush LSP request '{method}'"))?;
        pending.frame_written = true;
        drop(writer);

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(mut val)) => {
                if !self.is_alive.load(Ordering::Acquire) {
                    let details = self
                        .exit_details()
                        .await
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default();
                    anyhow::bail!(
                        "Language server process has exited while answering '{method}'{details}"
                    );
                }
                if let Some(busy) = busy {
                    val[BUSY_MEMBER] = serde_json::to_value(busy)?;
                }
                Ok(val)
            }
            Ok(Err(_)) => {
                let details = self
                    .exit_details()
                    .await
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "Language server process has exited while answering '{method}'{details}"
                )
            }
            Err(_) => {
                anyhow::bail!("Timeout waiting for response to '{method}'");
            }
        }
    }

    /// Send a notification to the language server.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        self.send_notification_for(DocumentOwner::Direct, method, params)
            .await
    }

    /// Send a notification owned by one gateway session. Ownership lets a disconnected
    /// client restore every overlay even when it never sends `didClose`.
    pub async fn send_session_notification(
        &self,
        session_id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<()> {
        self.send_notification_for(DocumentOwner::Session(session_id), method, params)
            .await
    }

    /// Restore or close every document a lost gateway session owned.
    pub async fn close_session(&self, session_id: u64) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.config.request_timeout;
        let uris = tokio::time::timeout_at(deadline, self.documents.lock())
            .await
            .context("Timeout waiting to close language-server session")?
            .sessions
            .get(&session_id)
            .cloned()
            .unwrap_or_default();
        for uri in uris {
            self.send_notification_for_until(
                DocumentOwner::Session(session_id),
                "textDocument/didClose",
                serde_json::json!({ "textDocument": { "uri": uri } }),
                deadline,
            )
            .await?;
        }
        Ok(())
    }
}
