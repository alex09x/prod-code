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
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;

use crate::diagnostics::annotate::annotate_missing_symbols;
use crate::diagnostics::filter::{
    refuse_unchecked, set_aside_derive_expansions, set_aside_preexisting, symbol_names,
};
use crate::diagnostics::parse::parse_items;
use crate::diagnostics::platform::{
    platform_excluded_report, platform_exclusion_reason, reconcile_swift_cross_target_diagnostics,
};
use crate::diagnostics::reexport::{
    display, resolved_tokens, suppress_used_public_reexport_warnings,
};
use crate::diagnostics::syntax::{
    ensure_source_file, is_parser_validated_file, validate_file_content,
};
use crate::diagnostics::types::DiagnosticsReport;
use crate::diagnostics::validate::single::on_disk;
use crate::session::LspSession;

/// Validates several proposed file contents together, the way a multi-file refactor must be
/// judged: every file is opened with its new text in one session (a private overlay on the
/// gateway), then diagnostics are pulled for each of them and for `also_check` (unchanged
/// files that may break, typically callers of an edited symbol). An edit in one file is
/// therefore checked against the proposed state of the others, not against the checkout.
/// Nothing is written anywhere.
pub async fn validate_texts(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
) -> Result<Vec<DiagnosticsReport>> {
    for file in edits.iter().map(|(file, _)| file).chain(also_check) {
        if !is_parser_validated_file(file) {
            ensure_source_file(file)?;
        }
    }
    // An extra file is checked against its text on disk; one that cannot be read would come back
    // as a clean report nobody made, and the change would pass unchecked there (#446).
    // Read and verify all also_check files up-front to fail closed in all branches.
    let mut also_texts = Vec::with_capacity(also_check.len());
    for file in also_check {
        let abs = if file.is_absolute() {
            file.clone()
        } else {
            root.join(file)
        };
        also_texts.push(std::fs::read_to_string(&abs).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}, which the change must be checked against; nothing was \
                 validated: {e}",
                abs.display()
            )
        })?);
    }
    // Fast path: if all files are parser-validated, validate locally without an LSP session (#733, #778).
    if edits.iter().all(|(f, _)| is_parser_validated_file(f))
        && also_check.iter().all(|f| is_parser_validated_file(f))
    {
        let mut reports = Vec::with_capacity(edits.len() + also_check.len());
        for (file, text) in edits {
            reports.push(validate_file_content(&display(root, file), file, text));
        }
        for (file, text) in also_check.iter().zip(&also_texts) {
            reports.push(validate_file_content(&display(root, file), file, text));
        }
        return Ok(reports);
    }

    let engines: HashSet<&'static str> = edits
        .iter()
        .map(|(p, _)| crate::lang::engine_group_for_path(p))
        .chain(
            also_check
                .iter()
                .map(|p| crate::lang::engine_group_for_path(p)),
        )
        .collect();

    // When a batch spans multiple languages, route each subset to its matching LSP engine
    // rather than asking one engine (e.g. gopls) for diagnostics on incompatible files (#751, #761).
    if engines.len() > 1 {
        let mut final_reports: Vec<Option<DiagnosticsReport>> =
            vec![None; edits.len() + also_check.len()];

        for &engine in &engines {
            if engine == "json" || engine == "markdown" || engine == "xml" {
                for (orig_i, (file, text)) in edits.iter().enumerate() {
                    if crate::lang::engine_group_for_path(file) == engine {
                        final_reports[orig_i] =
                            Some(validate_file_content(&display(root, file), file, text));
                    }
                }
                for (orig_j, file) in also_check.iter().enumerate() {
                    if crate::lang::engine_group_for_path(file) == engine {
                        final_reports[edits.len() + orig_j] = Some(validate_file_content(
                            &display(root, file),
                            file,
                            &also_texts[orig_j],
                        ));
                    }
                }
                continue;
            }

            let mut group_edits = Vec::new();
            let mut edit_indices = Vec::new();
            for (orig_i, edit) in edits.iter().enumerate() {
                if crate::lang::engine_group_for_path(&edit.0) == engine {
                    group_edits.push(edit.clone());
                    edit_indices.push(orig_i);
                }
            }

            let mut group_also = Vec::new();
            let mut group_also_texts = Vec::new();
            let mut also_indices = Vec::new();
            for (orig_j, file) in also_check.iter().enumerate() {
                if crate::lang::engine_group_for_path(file) == engine {
                    group_also.push(file.clone());
                    group_also_texts.push(also_texts[orig_j].clone());
                    also_indices.push(orig_j);
                }
            }

            if group_edits.is_empty() && group_also.is_empty() {
                continue;
            }

            let sub_reports = Box::pin(validate_texts_single_engine(
                remote,
                root,
                &group_edits,
                &group_also,
                &group_also_texts,
            ))
            .await?;

            for (sub_i, &orig_i) in edit_indices.iter().enumerate() {
                final_reports[orig_i] = Some(sub_reports[sub_i].clone());
            }
            for (sub_j, &orig_j) in also_indices.iter().enumerate() {
                final_reports[edits.len() + orig_j] =
                    Some(sub_reports[group_edits.len() + sub_j].clone());
            }
        }

        return Ok(final_reports
            .into_iter()
            .enumerate()
            .map(|(idx, r)| {
                r.unwrap_or_else(|| {
                    let file = if idx < edits.len() {
                        &edits[idx].0
                    } else {
                        &also_check[idx - edits.len()]
                    };
                    DiagnosticsReport {
                        file: display(root, file),
                        errors: 0,
                        warnings: 0,
                        items: Vec::new(),
                        preexisting: Vec::new(),
                        in_derive: Vec::new(),
                        auto_trait: Vec::new(),
                        hallucinations: Vec::new(),
                    }
                })
            })
            .collect());
    }

    validate_texts_single_engine(remote, root, edits, also_check, &also_texts).await
}

async fn validate_texts_single_engine(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
    also_texts: &[String],
) -> Result<Vec<DiagnosticsReport>> {
    let hint = edits
        .first()
        .map(|(file, _)| file.as_path())
        .or_else(|| also_check.first().map(|p| p.as_path()));
    // What every file says as it is on disk: an error the checkout already has is not the
    // edit's, and a report that counts it refuses every edit to that file.
    let mut baselines: HashMap<String, (DiagnosticsReport, String)> = HashMap::new();
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|&i| !crate::lang::is_header(&edits[i].0));

    // Rust discovers module contents through their parent file. Open nested source files first
    // so a newly added child exists in the overlay before its parent re-export is analyzed.
    let mut rust_order: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| edits[i].0.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .collect();
    rust_order.sort_by_key(|&i| {
        let file = &edits[i].0;
        let is_module_root = matches!(
            file.file_name().and_then(|name| name.to_str()),
            Some("lib.rs" | "main.rs" | "mod.rs")
        );
        std::cmp::Reverse(file.components().count().saturating_sub(if is_module_root {
            1
        } else {
            0
        }))
    });
    let mut rust_order = rust_order.into_iter();
    for i in &mut order {
        if edits[*i].0.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            *i = rust_order
                .next()
                .expect("one Rust edit per Rust order slot");
        }
    }

    // Capture the pre-edit symbols against the checkout, on the validation engine's unchanged
    // view. This has to finish before opening any proposal, since a new child module can change
    // what documentSymbol reports for its parent.
    let mut before_symbols = Vec::with_capacity(order.len());
    {
        let mut checkout = LspSession::open_for_validation(remote, root, hint).await?;
        for file in edits.iter().map(|(f, _)| f).chain(also_check) {
            let shown = display(root, file);
            if let Some(before) = on_disk(&mut checkout, root, file, &shown).await {
                baselines.insert(shown, before);
            }
        }
        for &i in &order {
            let file = &edits[i].0;
            if root.join(file).is_file() || file.is_file() {
                let uri = checkout.uri_for(file)?;
                before_symbols.push(
                    checkout
                        .query(
                            file,
                            "textDocument/documentSymbol",
                            serde_json::json!({ "textDocument": { "uri": uri } }),
                        )
                        .await
                        .map(|r| symbol_names(&r))
                        .unwrap_or_default(),
                );
            } else {
                before_symbols.push(BTreeSet::new());
            }
        }
        checkout.close().await;
    }

    let mut session = LspSession::open_for_validation(remote, root, hint).await?;
    let mut sources: HashMap<String, String> = HashMap::new();
    // clangd builds a source against the header text that is open when the source is built.
    // Open headers first so every source sees its proposed header (#292); then open every file
    // before asking for any after-set so new nested modules are visible to their parents.
    let mut proposed = Vec::with_capacity(order.len());
    for &i in &order {
        let (file, text) = &edits[i];
        let uri = session.open_text(file, text).await?;
        sources.insert(display(root, file), text.clone());
        proposed.push((i, file.clone(), uri));
    }

    // Compare only after the complete proposal is open. A re-export from a proposed child file
    // must not look removed just because that child was not open when the parent was queried.
    let mut missing: Vec<(String, String)> = Vec::new();
    for ((_, file, uri), before) in proposed.iter().zip(before_symbols) {
        let after = session
            .request(
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
            .map(|r| symbol_names(&r))
            .unwrap_or_default();
        let shown = display(root, file);
        for name in before.difference(&after) {
            missing.push((name.clone(), shown.clone()));
        }
    }

    // The proposal was opened in dependency order; reports keep the caller's edit order.
    let mut diagnostic_order: Vec<_> = proposed.iter().collect();
    diagnostic_order.sort_by_key(|(i, _, _)| *i);
    let mut reports = Vec::with_capacity(edits.len() + also_check.len());
    for (i, file, uri) in diagnostic_order {
        let diag_result = session
            .request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        let result = match diag_result {
            Ok(res) => res,
            Err(err) => {
                if let Some(reason) = platform_exclusion_reason(file, &err) {
                    let shown = display(root, file);
                    let report = platform_excluded_report(&shown, &reason);
                    debug_assert_eq!(reports.len(), *i);
                    reports.push(report);
                    continue;
                }
                return Err(err);
            }
        };
        let shown = display(root, file);
        let mut report = parse_items(&shown, &result);
        if let (Some((before, before_text)), Some(text)) =
            (baselines.get(&shown), sources.get(&shown))
        {
            set_aside_preexisting(&mut report, text, before, before_text);
        }
        if let Some(text) = sources.get(&shown) {
            set_aside_derive_expansions(&mut report, text);
        }
        refuse_unchecked(&mut report);
        debug_assert_eq!(reports.len(), *i);
        reports.push(report);
    }
    for (file, text) in also_check.iter().zip(also_texts) {
        let uri = session.uri_for(file)?;
        let diag_result = session
            .query(
                file,
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        let result = match diag_result {
            Ok(res) => res,
            Err(err) => {
                if let Some(reason) = platform_exclusion_reason(file, &err) {
                    let shown = display(root, file);
                    let report = platform_excluded_report(&shown, &reason);
                    reports.push(report);
                    continue;
                }
                return Err(err);
            }
        };
        let shown = display(root, file);
        let mut report = parse_items(&shown, &result);
        if let Some((before, before_text)) = baselines.get(&shown) {
            set_aside_preexisting(&mut report, text, before, before_text);
        }
        set_aside_derive_expansions(&mut report, text);
        refuse_unchecked(&mut report);
        sources.insert(shown.clone(), text.clone());
        reports.push(report);
    }
    suppress_used_public_reexport_warnings(&mut session, root, edits.len(), &mut reports, &sources)
        .await;
    let resolved = resolved_tokens(&mut session, root, &reports, &sources, &missing).await;
    session.close().await;
    annotate_missing_symbols(&mut reports, &sources, &missing, &resolved);
    let _ = reconcile_swift_cross_target_diagnostics(remote, root, edits, &mut reports).await;
    Ok(reports)
}
