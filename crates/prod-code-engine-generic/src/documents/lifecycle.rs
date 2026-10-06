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
use std::sync::atomic::Ordering;

use crate::engine::GenericLspEngine;
use crate::types::{DocumentLifecycle, DocumentOwner, DocumentState, lock_unpoisoned};

pub(crate) fn apply_content_changes(
    base: Option<&str>,
    params: &serde_json::Value,
) -> Result<String> {
    let changes = params
        .get("contentChanges")
        .and_then(|v| v.as_array())
        .context("textDocument/didChange has no contentChanges array")?;
    let mut text = base.map(str::to_string);
    for change in changes {
        let replacement = change
            .get("text")
            .and_then(|v| v.as_str())
            .context("textDocument/didChange change has no text")?;
        if let Some(range) = change.get("range") {
            let current = text
                .as_mut()
                .context("incremental change requires this owner's open document")?;
            let start = change_offset(current, &range["start"])?;
            let end = change_offset(current, &range["end"])?;
            anyhow::ensure!(start <= end, "incremental change end precedes its start");
            current.replace_range(start..end, replacement);
        } else {
            text = Some(replacement.to_string());
        }
    }
    text.context("empty contentChanges requires this owner's open document")
}

pub(crate) fn change_offset(text: &str, position: &serde_json::Value) -> Result<usize> {
    let coordinate = |name: &str| -> Result<u32> {
        position
            .get(name)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .with_context(|| format!("incremental change has invalid '{name}' coordinate"))
    };
    let line = coordinate("line")?;
    let character = coordinate("character")? as usize;
    let mut start = 0;
    for _ in 0..line {
        start += text[start..]
            .find('\n')
            .context("incremental change line is outside the document")?
            + 1;
    }
    let tail = &text[start..];
    let content = match tail.find('\n') {
        Some(end) => tail[..end].strip_suffix('\r').unwrap_or(&tail[..end]),
        None => tail,
    };
    let mut units = 0;
    for (byte, ch) in content.char_indices() {
        if units == character {
            return Ok(start + byte);
        }
        units += ch.len_utf16();
        anyhow::ensure!(
            units <= character,
            "incremental change splits a UTF-16 surrogate pair"
        );
    }
    anyhow::ensure!(
        units == character,
        "incremental change column is outside the line"
    );
    Ok(start + content.len())
}

pub(crate) fn next_order(documents: &mut DocumentLifecycle) -> Result<u64> {
    documents.next_order = documents
        .next_order
        .checked_add(1)
        .context("document ownership order overflowed")?;
    Ok(documents.next_order)
}

pub(crate) fn next_version(version: i64, uri: &str) -> Result<i64> {
    version
        .checked_add(1)
        .with_context(|| format!("document version overflowed for {uri}"))
}

pub(crate) fn changed(uri: &str, version: i64, text: String) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": uri, "version": version },
        "contentChanges": [{ "text": text }]
    })
}

pub(crate) fn visible_owner(document: &DocumentState) -> Option<DocumentOwner> {
    document
        .owners
        .iter()
        .max_by_key(|(_, owned)| owned.order)
        .map(|(owner, _)| *owner)
}

pub(crate) fn record_session_owner(
    documents: &mut DocumentLifecycle,
    owner: DocumentOwner,
    uri: &str,
) {
    if let DocumentOwner::Session(session) = owner {
        documents
            .sessions
            .entry(session)
            .or_default()
            .insert(uri.to_string());
    }
}

pub(crate) fn forget_session_owner(
    documents: &mut DocumentLifecycle,
    owner: DocumentOwner,
    uri: &str,
) {
    let DocumentOwner::Session(session) = owner else {
        return;
    };
    if let Some(opened) = documents.sessions.get_mut(&session) {
        opened.remove(uri);
        if opened.is_empty() {
            documents.sessions.remove(&session);
        }
    }
}

impl GenericLspEngine {
    pub(crate) fn disk_text(&self, uri: &str) -> Option<String> {
        url::Url::parse(uri)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .filter(|path| path.starts_with(&self.workspace_root))
            .and_then(|path| std::fs::read_to_string(path).ok())
    }

    pub(crate) fn retire_full_generation(&self, documents: &DocumentLifecycle) {
        let retained = documents
            .documents
            .values()
            .filter(|document| document.owners.is_empty())
            .count();
        if retained >= self.config.max_retained_documents.max(1) {
            self.accepts_documents.store(false, Ordering::Relaxed);
        }
    }

    /// Whether this server can safely accept another document generation. False means the
    /// whole engine must be evicted; reusing one retained identity after `didClose` is unsafe.
    pub fn accepts_documents(&self) -> bool {
        self.accepts_documents.load(Ordering::Relaxed)
    }

    pub(crate) fn invalidate_document_generation(&self) {
        self.accepts_documents.store(false, Ordering::Release);
        self.is_alive.store(false, Ordering::Release);
        let _ = lock_unpoisoned(&self._child).start_kill();
        lock_unpoisoned(&self.pending_requests).clear();
        lock_unpoisoned(&self.health_probe_pending).take();
    }
}
