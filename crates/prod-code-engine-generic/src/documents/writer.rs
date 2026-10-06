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
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::diagnostics::Sent;
use crate::engine::GenericLspEngine;
use crate::reader::write_frame_until;

impl GenericLspEngine {
    pub(crate) async fn record_and_write_notification_until(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        // Recorded before the text is on its way, so no publication for it can come first.
        if let Some(uri) = params.pointer("/textDocument/uri").and_then(|u| u.as_str()) {
            // A publication from before an open or a close describes a text that is gone:
            // another session's, which numbered its versions from 1 as this one does.
            if matches!(method, "textDocument/didOpen" | "textDocument/didClose") {
                tokio::time::timeout_at(deadline, self.diagnostics.write())
                    .await
                    .with_context(|| {
                        format!("Timeout recording diagnostics for notification '{method}'")
                    })?
                    .remove(uri);
            }
            match method {
                "textDocument/didOpen" | "textDocument/didChange" => {
                    let version = params
                        .pointer("/textDocument/version")
                        .and_then(|v| v.as_i64());
                    tokio::time::timeout_at(deadline, self.sent.write())
                        .await
                        .with_context(|| {
                            format!("Timeout recording sent text for notification '{method}'")
                        })?
                        .insert(
                            uri.to_string(),
                            Sent {
                                version,
                                at: Instant::now(),
                            },
                        );
                }
                "textDocument/didClose" => {
                    tokio::time::timeout_at(deadline, self.sent.write())
                        .await
                        .with_context(|| {
                            format!("Timeout recording closed text for notification '{method}'")
                        })?
                        .remove(uri);
                }
                _ => {}
            }
        }
        self.write_notification_until(method, params, deadline)
            .await
    }

    pub(crate) async fn write_notification_until(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        if !self.is_alive.load(Ordering::Acquire) {
            let details = self
                .exit_details()
                .await
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            anyhow::bail!(
                "Language server process has exited before notification '{method}'{details}"
            );
        }
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
}
