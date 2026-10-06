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
use std::path::Path;

use super::worker::BackendWorker;

impl BackendWorker {
    /// Perform the one-time LSP initialize handshake with the backend worker.
    pub(crate) async fn initialize_backend(&self, workspace_root: &Path) -> Result<()> {
        let ws_uri = prod_code_protocol::path::file_uri(workspace_root);
        let ws_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");

        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": ws_uri,
                "workspaceFolders": [
                    {
                        "name": ws_name,
                        "uri": ws_uri
                    }
                ],
                "capabilities": {
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
            }
        });

        let mut rx = self.subscribe();
        self.send_lsp(&init_req.to_string()).await?;

        // Server requests have their own IDs and may collide with our initialize request.
        // Only a response with the complete capabilities object establishes a usable server.
        loop {
            anyhow::ensure!(self.is_alive(), "backend exited during initialization");
            let message = tokio::select! {
                _ = self.closed.notified() => anyhow::bail!("backend exited during initialization"),
                message = rx.recv() => message.context("backend initialization response stream closed")?,
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
                continue;
            };
            if value.get("method").is_some()
                || value.get("id").and_then(serde_json::Value::as_u64) != Some(1)
            {
                continue;
            }
            if let Some(error) = value.get("error") {
                anyhow::bail!("backend refused initialization: {error}");
            }
            let capabilities = value
                .pointer("/result/capabilities")
                .filter(|capabilities| capabilities.is_object())
                .context("backend initialize response has no capabilities object")?;
            *self.capabilities.write().await = Some(capabilities.clone());
            break;
        }

        // Send initialized notification
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        self.send_lsp(&initialized.to_string()).await?;

        tracing::info!(workspace = %workspace_root.display(), "Backend language server initialized and warm");
        Ok(())
    }
}
