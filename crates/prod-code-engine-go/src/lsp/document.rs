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
use std::sync::Arc;

use crate::engine::GoEngine;

impl GoEngine {
    /// Acquire the per-URI document lifecycle lock to serialize state updates and LSP notification writes.
    pub(crate) async fn lock_document(&self, file_uri: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.file_locks.lock().await;
        locks.entry(file_uri.to_string()).or_default().clone()
    }

    /// Notify `gopls` that a document was opened in an editor or worktree.
    /// If the document is already open, deduplicate by sending `textDocument/didChange`
    /// instead of a duplicate `didOpen` that churns gopls package loading and triggers re-indexing.
    /// State transitions and LSP notification writes are serialized per-URI so concurrent calls
    /// cannot reorder or desynchronize `gopls` from `open_files`.
    pub async fn did_open(&self, file_uri: &str, text: &str) -> Result<()> {
        let doc_lock = self.lock_document(file_uri).await;
        let _guard = doc_lock.lock().await;

        let (is_open, version) = {
            let mut open = self.open_files.write().await;
            if let Some(version) = open.get_mut(file_uri) {
                *version = version.wrapping_add(1);
                (true, *version)
            } else {
                open.insert(file_uri.to_string(), 1);
                (false, 1)
            }
        };

        let result = if is_open {
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
        } else {
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
        };

        if result.is_err() && !is_open {
            let mut open = self.open_files.write().await;
            open.remove(file_uri);
        }
        result
    }

    /// Notify `gopls` of an unsaved text buffer update.
    pub async fn did_change(&self, file_uri: &str, text: &str, version: i32) -> Result<()> {
        let doc_lock = self.lock_document(file_uri).await;
        let _guard = doc_lock.lock().await;

        let prior_version = {
            let mut open = self.open_files.write().await;
            open.insert(file_uri.to_string(), version)
        };

        let result = self
            .send_notification(
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
            .await;

        if result.is_err() {
            let mut open = self.open_files.write().await;
            if let Some(pv) = prior_version {
                open.insert(file_uri.to_string(), pv);
            } else {
                open.remove(file_uri);
            }
        }
        result
    }

    /// Notify `gopls` that a document was closed.
    pub async fn did_close(&self, file_uri: &str) -> Result<()> {
        let doc_lock = self.lock_document(file_uri).await;
        let _guard = doc_lock.lock().await;

        let prior_version = {
            let mut open = self.open_files.write().await;
            open.remove(file_uri)
        };

        if prior_version.is_none() {
            return Ok(());
        }

        let result = self
            .send_notification(
                "textDocument/didClose",
                serde_json::json!({
                    "textDocument": {
                        "uri": file_uri
                    }
                }),
            )
            .await;

        if result.is_err() {
            if let Some(pv) = prior_version {
                let mut open = self.open_files.write().await;
                open.insert(file_uri.to_string(), pv);
            }
        } else {
            let mut locks = self.file_locks.lock().await;
            if Arc::strong_count(&doc_lock) <= 2 {
                locks.remove(file_uri);
            }
        }
        result
    }
}
