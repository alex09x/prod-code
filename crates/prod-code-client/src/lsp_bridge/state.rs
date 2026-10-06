/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::encoding::lsp_offset;
use super::transport::PendingRequests;
use std::collections::HashMap;

#[derive(Default, Debug, Clone)]
pub struct TrackedDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

#[derive(Default, Debug, Clone)]
pub struct LspStateTracker {
    pub initialize_req: Option<String>,
    pub initialized_sent: bool,
    pub configuration_notifications: Vec<String>,
    pub open_documents: HashMap<String, TrackedDocument>,
}

impl LspStateTracker {
    pub fn record_client_message(&mut self, raw: &str, position_encoding: u8) {
        let method = prod_code_client::editor_files::method_of(raw);
        let Some(method_str) = method.as_deref() else {
            return;
        };

        match method_str {
            "initialize" => {
                self.initialize_req = Some(raw.to_string());
            }
            "initialized" => {
                self.initialized_sent = true;
            }
            "workspace/didChangeConfiguration" => {
                self.configuration_notifications.push(raw.to_string());
            }
            "textDocument/didOpen" => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(td) = val.pointer("/params/textDocument") {
                        let uri = td
                            .get("uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let language_id = td
                            .get("languageId")
                            .and_then(|l| l.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let version = td.get("version").and_then(|v| v.as_i64()).unwrap_or(1);
                        let text = td
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if !uri.is_empty() {
                            self.open_documents.insert(
                                uri.clone(),
                                TrackedDocument {
                                    uri,
                                    language_id,
                                    version,
                                    text,
                                },
                            );
                        }
                    }
                }
            }
            "textDocument/didClose" => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(uri) = val
                        .pointer("/params/textDocument/uri")
                        .and_then(|u| u.as_str())
                    {
                        self.open_documents.remove(uri);
                    }
                }
            }
            "textDocument/didChange" => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(params) = val.get("params") {
                        if let Some(uri) =
                            params.pointer("/textDocument/uri").and_then(|u| u.as_str())
                        {
                            if let Some(doc) = self.open_documents.get_mut(uri) {
                                if let Some(version) = params
                                    .pointer("/textDocument/version")
                                    .and_then(|v| v.as_i64())
                                {
                                    doc.version = version;
                                }
                                if let Some(changes) =
                                    params.get("contentChanges").and_then(|c| c.as_array())
                                {
                                    for change in changes {
                                        if let Some(replacement) =
                                            change.get("text").and_then(|t| t.as_str())
                                        {
                                            if let Some(range) = change.get("range") {
                                                if let (
                                                    Some(start_line),
                                                    Some(start_col),
                                                    Some(end_line),
                                                    Some(end_col),
                                                ) = (
                                                    range
                                                        .pointer("/start/line")
                                                        .and_then(|l| l.as_u64()),
                                                    range
                                                        .pointer("/start/character")
                                                        .and_then(|c| c.as_u64()),
                                                    range
                                                        .pointer("/end/line")
                                                        .and_then(|l| l.as_u64()),
                                                    range
                                                        .pointer("/end/character")
                                                        .and_then(|c| c.as_u64()),
                                                ) {
                                                    let start_off = lsp_offset(
                                                        &doc.text,
                                                        start_line as usize,
                                                        start_col as usize,
                                                        position_encoding,
                                                    );
                                                    let end_off = lsp_offset(
                                                        &doc.text,
                                                        end_line as usize,
                                                        end_col as usize,
                                                        position_encoding,
                                                    );
                                                    if start_off <= end_off
                                                        && end_off <= doc.text.len()
                                                    {
                                                        doc.text.replace_range(
                                                            start_off..end_off,
                                                            replacement,
                                                        );
                                                    }
                                                }
                                            } else {
                                                doc.text = replacement.to_string();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn is_idempotent_lsp_request(method: &str) -> bool {
    matches!(
        method,
        "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/declaration"
            | "textDocument/typeDefinition"
            | "textDocument/implementation"
            | "textDocument/references"
            | "textDocument/documentHighlight"
            | "textDocument/documentSymbol"
            | "textDocument/codeAction"
            | "textDocument/codeLens"
            | "codeLens/resolve"
            | "textDocument/documentLink"
            | "documentLink/resolve"
            | "textDocument/documentColor"
            | "textDocument/colorPresentation"
            | "textDocument/formatting"
            | "textDocument/rangeFormatting"
            | "textDocument/onTypeFormatting"
            | "textDocument/prepareRename"
            | "textDocument/foldingRange"
            | "textDocument/selectionRange"
            | "textDocument/signatureHelp"
            | "textDocument/completion"
            | "completionItem/resolve"
            | "textDocument/semanticTokens"
            | "textDocument/semanticTokens/full"
            | "textDocument/semanticTokens/full/delta"
            | "textDocument/semanticTokens/range"
            | "textDocument/inlayHint"
            | "inlayHint/resolve"
            | "textDocument/inlineValue"
            | "textDocument/moniker"
            | "textDocument/prepareCallHierarchy"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "textDocument/prepareTypeHierarchy"
            | "typeHierarchy/supertypes"
            | "typeHierarchy/subtypes"
            | "workspace/symbol"
            | "workspace/symbol/resolve"
            | "workspace/diagnostic"
            | "textDocument/diagnostic"
    )
}

pub async fn fail_pending_requests(
    pending_requests: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
    error_msg: &str,
) {
    let pending = {
        let mut lock = pending_requests.lock().await;
        std::mem::take(&mut *lock)
    };
    if pending.is_empty() {
        return;
    }
    let mut stdout = editor_out.lock().await;
    for (id, _method, _) in pending {
        let err_resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32097,
                "message": format!("prod-code lsp: gateway connection lost ({error_msg})"),
            }
        });
        let _ = prod_code_client::editor_files::write_frame(&mut *stdout, &err_resp.to_string()).await;
    }
}
