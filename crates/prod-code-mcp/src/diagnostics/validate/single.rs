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
use std::net::SocketAddr;
use std::path::Path;

use crate::diagnostics::filter::{
    refuse_unchecked, set_aside_derive_expansions, set_aside_preexisting,
};
use crate::diagnostics::parse::parse_items;
use crate::diagnostics::platform::reconcile_swift_cross_target_diagnostics;
use crate::diagnostics::reexport::display;
use crate::diagnostics::syntax::{
    ensure_source_file, is_parser_validated_file, validate_file_content,
};
use crate::diagnostics::types::DiagnosticsReport;
use crate::session::LspSession;

/// The diagnostics of `file` as it is on disk, and that text, or `None` for a file that does
/// not exist yet.
///
/// Asked of the main engine, in a session of its own, never of the validation engine: the main
/// engine holds the checkout's state warm, so this costs what any diagnostics query costs. The
/// validation engine is left holding only proposals, and one that repeats the last proposal —
/// the same dry run asked twice — finds everything it needs still computed (#73).
pub async fn on_disk(
    session: &mut LspSession,
    root: &Path,
    file: &Path,
    shown: &str,
) -> Option<(DiagnosticsReport, String)> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let text = std::fs::read_to_string(&abs).ok()?;
    let uri = session.uri_for(file).ok()?;
    let query_params = serde_json::json!({ "textDocument": { "uri": uri } });
    let result = match session
        .query(file, "textDocument/diagnostic", query_params.clone())
        .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                error = %err,
                file = %file.display(),
                "initial on_disk diagnostic query failed, retrying once after backoff"
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            session
                .query(file, "textDocument/diagnostic", query_params)
                .await
                .ok()?
        }
    };
    Some((parse_items(shown, &result), text))
}

/// Diagnostics of `file` as it is on disk.
pub async fn diagnostics(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<DiagnosticsReport> {
    if is_parser_validated_file(file) {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            root.join(file)
        };
        let text = std::fs::read_to_string(&abs)
            .with_context(|| format!("cannot read {}", file.display()))?;
        return Ok(validate_file_content(&display(root, file), file, &text));
    }
    ensure_source_file(file)?;
    let mut session = LspSession::open(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let result = session
        .query(
            file,
            "textDocument/diagnostic",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    session.close().await;
    let mut report = parse_items(&display(root, file), &result);
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    // What validation sets aside, a file on disk sets aside too (#327).
    if let Ok(text) = std::fs::read_to_string(&abs) {
        set_aside_derive_expansions(&mut report, &text);
    }
    Ok(report)
}

/// Diagnostics of `file` as if its content were `new_text`; nothing is written.
pub async fn validate_text(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    new_text: &str,
) -> Result<DiagnosticsReport> {
    if is_parser_validated_file(file) {
        return Ok(validate_file_content(&display(root, file), file, new_text));
    }
    ensure_source_file(file)?;
    let shown = display(root, file);
    // The file as it is on disk, read on the validation engine: it has no overlay for this
    // session, and it is the engine the gateway warms. The main engine is cold for the file's
    // diagnostics after a restart, and asking it cost 21 s of a 24 s validation (#235).
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let before = if abs.is_file() {
        let mut checkout = LspSession::open_for_validation(remote, root, Some(file)).await?;
        let before = on_disk(&mut checkout, root, file, &shown).await;
        checkout.close().await;
        before
    } else {
        None
    };
    let mut session = LspSession::open_for_validation(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let params = serde_json::json!({ "textDocument": { "uri": uri } });
    let result = session
        .query_with_text(file, new_text, "textDocument/diagnostic", params)
        .await?;
    session.close().await;
    let mut report = parse_items(&shown, &result);
    if let Some((before, before_text)) = before {
        set_aside_preexisting(&mut report, new_text, &before, &before_text);
    }
    set_aside_derive_expansions(&mut report, new_text);
    refuse_unchecked(&mut report);
    if file.extension().and_then(|e| e.to_str()) == Some("swift") && report.errors > 0 {
        let mut reports = vec![report];
        let edits = [(file.to_path_buf(), new_text.to_string())];
        let _ = reconcile_swift_cross_target_diagnostics(remote, root, &edits, &mut reports).await;
        report = reports.pop().unwrap();
    }
    Ok(report)
}
