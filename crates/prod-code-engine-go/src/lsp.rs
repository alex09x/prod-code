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
use prod_code_protocol::readiness::{BUSY_MEMBER, INDEX_WAIT, needs_index};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::engine::GoEngine;
use crate::reader::write_frame_until;
use crate::types::{
    InitializationGuard, OrdinaryActivity, PendingRequest, lock_unpoisoned,
    validate_initialize_response,
};

/// gopls v0.23.0 sends a complete diagnostic report with an empty discriminator (#475).
/// Canonicalize only that shape. Missing/malformed items and unchanged reports still reach the
/// client unchanged so its strict evidence checks can reject them; errors are never hidden.
pub(crate) fn normalize_diagnostic_response(method: &str, reply: &mut serde_json::Value) {
    if method == "textDocument/diagnostic"
        && reply.get("error").is_none()
        && reply["result"]["kind"].as_str() == Some("")
        && reply["result"]["items"].is_array()
    {
        reply["result"]["kind"] = serde_json::json!("full");
    }
}

impl GoEngine {
    /// Perform the one-time LSP initialize handshake with `gopls`.
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        let mut handshake = InitializationGuard {
            engine: self,
            complete: false,
        };
        *self.capabilities.write().await = None;
        let ws_str = self.workspace_root.to_string_lossy().to_string();
        let ws_name = self
            .workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("go-workspace");

        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", ws_str),
            "workspaceFolders": [
                {
                    "name": ws_name,
                    "uri": format!("file://{}", ws_str)
                }
            ],
            "capabilities": {
                // gopls reports loading its packages as progress to a client that takes it.
                "window": {
                    "workDoneProgress": true
                },
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                },
                "textDocument": {
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

        let resp = self.send_request("initialize", init_params).await?;

        let capabilities = validate_initialize_response(&resp, "gopls")?;

        // Send initialized notification as required by LSP spec
        self.send_notification("initialized", serde_json::json!({}))
            .await?;
        *self.capabilities.write().await = Some(capabilities);
        self.readiness.started();
        handshake.complete = true;

        Ok(resp)
    }

    /// Send a typed JSON-RPC request and wait for the response.
    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before request '{method}'");
        }
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        // A question answered from the index waits until gopls has loaded its packages (#391).
        let busy = if needs_index(method) {
            self.readiness.wait(INDEX_WAIT).await
        } else {
            None
        };
        let deadline = tokio::time::Instant::now() + self.request_timeout;
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
            .with_context(|| format!("Timeout waiting to send gopls request '{method}'"))?;
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before request '{method}'");
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
            .with_context(|| format!("Timeout writing gopls request '{method}'"))?
            .with_context(|| format!("Failed to write gopls request '{method}'"))?;
        tokio::time::timeout_at(deadline, writer.flush())
            .await
            .with_context(|| format!("Timeout flushing gopls request '{method}'"))?
            .with_context(|| format!("Failed to flush gopls request '{method}'"))?;
        pending.frame_written = true;
        drop(writer);

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(mut val)) => {
                if !self.is_alive.load(Ordering::Acquire) {
                    anyhow::bail!("gopls has exited while answering '{method}'");
                }
                normalize_diagnostic_response(method, &mut val);
                if let Some(busy) = busy {
                    val[BUSY_MEMBER] = serde_json::to_value(busy)?;
                }
                Ok(val)
            }
            Ok(Err(_)) => anyhow::bail!("gopls has exited while answering '{method}'"),
            Err(_) => {
                anyhow::bail!("Timeout waiting for gopls response to method '{method}'");
            }
        }
    }

    /// Send an asynchronous JSON-RPC notification to `gopls`.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before notification '{method}'");
        }
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        write_frame_until(
            &self.stdin,
            &payload,
            deadline,
            method,
            &Arc::downgrade(&self._child),
            &Arc::downgrade(&self.pending_requests),
            &Arc::downgrade(&self.is_alive),
        )
        .await
    }

    /// Notify `gopls` that a document was opened in an editor or worktree.
    pub async fn did_open(&self, file_uri: &str, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": "go",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .await
    }

    /// Notify `gopls` of an unsaved text buffer update.
    pub async fn did_change(&self, file_uri: &str, text: &str, version: i32) -> Result<()> {
        self.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "version": version
                },
                "contentChanges": [
                    { "text": text }
                ]
            }),
        )
        .await
    }

    /// Notify `gopls` that a document was closed.
    pub async fn did_close(&self, file_uri: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri
                }
            }),
        )
        .await
    }

    /// Query symbol definition.
    pub async fn definition(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<serde_json::Value>> {
        let resp = self
            .send_request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned())
    }

    /// Query hover documentation and type signatures.
    pub async fn hover(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<serde_json::Value>> {
        let resp = self
            .send_request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned())
    }

    /// Query all references to a symbol across the Go workspace.
    pub async fn references(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<serde_json::Value> {
        let resp = self
            .send_request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned().unwrap_or(serde_json::json!([])))
    }

    /// Query document symbols outline.
    pub async fn document_symbols(&self, file_uri: &str) -> Result<serde_json::Value> {
        let resp = self
            .send_request(
                "textDocument/documentSymbol",
                serde_json::json!({
                    "textDocument": { "uri": file_uri }
                }),
            )
            .await?;

        if let Some(err) = resp.get("error") {
            anyhow::bail!("gopls documentSymbol failed: {err}");
        }
        Ok(resp.get("result").cloned().unwrap_or(serde_json::json!([])))
    }
}
