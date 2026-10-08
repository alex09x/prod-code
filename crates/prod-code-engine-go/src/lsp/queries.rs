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

use crate::engine::GoEngine;

impl GoEngine {
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
