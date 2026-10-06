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
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::documents::lifecycle::{
    apply_content_changes, changed, forget_session_owner, next_order, next_version,
    record_session_owner, visible_owner,
};
use crate::engine::GenericLspEngine;
use crate::types::{
    DocumentLifecycle, DocumentMutation, DocumentOwner, DocumentState, OrdinaryActivity, OwnedText,
};

impl GenericLspEngine {
    pub(crate) async fn send_notification_for(
        &self,
        owner: DocumentOwner,
        method: &str,
        params: serde_json::Value,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.config.request_timeout;
        self.send_notification_for_until(owner, method, params, deadline)
            .await
    }

    pub(crate) async fn send_notification_for_until(
        &self,
        owner: DocumentOwner,
        method: &str,
        mut params: serde_json::Value,
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
        let _activity = OrdinaryActivity::begin(&self.ordinary_activity, &self.ordinary_epoch);
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(|u| u.as_str())
            .map(str::to_string);
        if method != "workspace/didChangeWatchedFiles" && uri.is_none() {
            return self
                .write_notification_until(method, params, deadline)
                .await;
        }
        // The lifecycle and the frame are serialized together. A disconnect can therefore
        // never clean up ownership before its last open/change has recorded that ownership.
        let mut documents = tokio::time::timeout_at(deadline, self.documents.lock())
            .await
            .with_context(|| {
                format!("Timeout waiting to update document state for notification '{method}'")
            })?;
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
        let mut mutation = DocumentMutation {
            engine: self,
            changed: false,
            committed: false,
        };
        if method == "workspace/didChangeWatchedFiles" {
            let result = async {
                self.refresh_retained_documents(
                    &mut documents,
                    &params,
                    deadline,
                    &mut mutation.changed,
                )
                .await?;
                self.write_notification_until(method, params, deadline)
                    .await
            }
            .await;
            if result.is_ok() {
                mutation.committed = true;
            }
            return result;
        }
        let uri = uri.expect("document notification URI checked above");
        let mut sent_method = method;
        match method {
            "textDocument/didOpen" => {
                if !self.accepts_documents.load(Ordering::Relaxed) {
                    anyhow::bail!(
                        "language server retained-document generation is full; restart it before opening {uri}"
                    );
                }
                let text = params
                    .pointer("/textDocument/text")
                    .and_then(|text| text.as_str())
                    .context("textDocument/didOpen has no text")?
                    .to_string();
                let on_disk = self.disk_text(&uri).is_some();
                if let Some(current_version) = documents
                    .documents
                    .get(&uri)
                    .map(|document| document.version)
                {
                    let version = next_version(current_version, &uri)?;
                    let order = next_order(&mut documents)?;
                    mutation.changed = true;
                    let document = documents.documents.get_mut(&uri).expect("document exists");
                    document.version = version;
                    document.owners.insert(
                        owner,
                        OwnedText {
                            text: text.clone(),
                            order,
                        },
                    );
                    if on_disk {
                        document.existed_on_disk = true;
                    }
                    sent_method = "textDocument/didChange";
                    params = changed(&uri, version, text);
                } else {
                    let version = params
                        .pointer("/textDocument/version")
                        .and_then(|version| version.as_i64())
                        .unwrap_or(1);
                    let order = next_order(&mut documents)?;
                    mutation.changed = true;
                    documents.documents.insert(
                        uri.clone(),
                        DocumentState {
                            version,
                            owners: HashMap::from([(owner, OwnedText { text, order })]),
                            existed_on_disk: on_disk,
                        },
                    );
                }
                record_session_owner(&mut documents, owner, &uri);
            }
            "textDocument/didChange" => {
                let base = documents
                    .documents
                    .get(&uri)
                    .and_then(|document| document.owners.get(&owner))
                    .map(|owned| owned.text.as_str());
                // Ranges belong to this owner's text, even while another owner's overlay is
                // visible. Compose every change before committing state or sending a frame.
                let text = apply_content_changes(base, &params)?;
                let on_disk = self.disk_text(&uri).is_some();
                if let Some(current_version) = documents
                    .documents
                    .get(&uri)
                    .map(|document| document.version)
                {
                    let version = next_version(current_version, &uri)?;
                    let order = next_order(&mut documents)?;
                    mutation.changed = true;
                    let document = documents.documents.get_mut(&uri).expect("document exists");
                    document.version = version;
                    document.owners.insert(
                        owner,
                        OwnedText {
                            text: text.clone(),
                            order,
                        },
                    );
                    if on_disk {
                        document.existed_on_disk = true;
                    }
                    params = changed(&uri, version, text);
                } else {
                    let version = params
                        .pointer("/textDocument/version")
                        .and_then(|version| version.as_i64())
                        .unwrap_or(1);
                    let order = next_order(&mut documents)?;
                    mutation.changed = true;
                    documents.documents.insert(
                        uri.clone(),
                        DocumentState {
                            version,
                            owners: HashMap::from([(
                                owner,
                                OwnedText {
                                    text: text.clone(),
                                    order,
                                },
                            )]),
                            existed_on_disk: on_disk,
                        },
                    );
                    params = changed(&uri, version, text);
                }
                record_session_owner(&mut documents, owner, &uri);
            }
            "textDocument/didClose" => {
                let Some(document) = documents.documents.get(&uri) else {
                    return self
                        .write_notification_until(method, params, deadline)
                        .await;
                };
                let was_visible = visible_owner(document) == Some(owner);
                if !document.owners.contains_key(&owner) {
                    return Ok(());
                }
                let existed_on_disk = document.existed_on_disk;
                let replacement = was_visible
                    .then(|| {
                        document
                            .owners
                            .iter()
                            .filter(|(candidate, _)| **candidate != owner)
                            .max_by_key(|(_, owned)| owned.order)
                            .map(|(_, owned)| owned.text.clone())
                    })
                    .flatten();
                let disk = (was_visible
                    && replacement.is_none()
                    && self.config.retain_open_documents
                    && self.accepts_documents.load(Ordering::Relaxed))
                .then(|| self.disk_text(&uri))
                .flatten();
                let next = if replacement.is_some() || disk.is_some() {
                    Some(next_version(document.version, &uri)?)
                } else {
                    None
                };
                mutation.changed = true;
                documents
                    .documents
                    .get_mut(&uri)
                    .expect("document checked above")
                    .owners
                    .remove(&owner);
                forget_session_owner(&mut documents, owner, &uri);
                if !was_visible {
                    mutation.committed = true;
                    return Ok(());
                }
                if let Some(text) = replacement {
                    let document = documents.documents.get_mut(&uri).expect("document exists");
                    let version = next.expect("replacement has a version");
                    document.version = version;
                    sent_method = "textDocument/didChange";
                    params = changed(&uri, version, text);
                } else if let Some(text) = disk {
                    let document = documents.documents.get_mut(&uri).expect("document exists");
                    let version = next.expect("retained disk text has a version");
                    document.version = version;
                    sent_method = "textDocument/didChange";
                    params = changed(&uri, version, text);
                    self.retire_full_generation(&documents);
                } else {
                    documents.documents.remove(&uri);
                    if self.config.retain_open_documents && existed_on_disk {
                        self.accepts_documents.store(false, Ordering::Relaxed);
                    }
                }
            }
            _ => {
                return self
                    .write_notification_until(method, params, deadline)
                    .await;
            }
        }
        let result = self
            .record_and_write_notification_until(sent_method, params, deadline)
            .await;
        if result.is_ok() {
            mutation.committed = true;
        }
        result
    }

    async fn refresh_retained_documents(
        &self,
        documents: &mut DocumentLifecycle,
        params: &serde_json::Value,
        deadline: tokio::time::Instant,
        mutated: &mut bool,
    ) -> Result<()> {
        let changes: Vec<(String, u64)> = params
            .get("changes")
            .and_then(|changes| changes.as_array())
            .into_iter()
            .flatten()
            .filter_map(|change| {
                Some((
                    change.get("uri")?.as_str()?.to_string(),
                    change.get("type")?.as_u64()?,
                ))
            })
            .collect();
        for (uri, kind) in changes {
            let Some(document) = documents.documents.get_mut(&uri) else {
                continue;
            };
            if !document.owners.is_empty() {
                continue;
            }
            if kind == 3 {
                *mutated = true;
                documents.documents.remove(&uri);
                self.accepts_documents.store(false, Ordering::Relaxed);
                self.record_and_write_notification_until(
                    "textDocument/didClose",
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                    deadline,
                )
                .await?;
            } else if let Some(text) = self.disk_text(&uri) {
                let version = next_version(document.version, &uri)?;
                *mutated = true;
                document.version = version;
                self.record_and_write_notification_until(
                    "textDocument/didChange",
                    changed(&uri, version, text),
                    deadline,
                )
                .await?;
            }
        }
        Ok(())
    }
}
