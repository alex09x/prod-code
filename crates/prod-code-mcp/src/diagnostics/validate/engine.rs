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
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
use crate::diagnostics::types::DiagnosticsReport;
use crate::session::LspSession;

use super::budget::{ValidationBudget, incomplete_report};

pub(super) async fn validate_texts_single_engine(
    remote: SocketAddr,
    root: &Path,
    edits: &[(PathBuf, String)],
    also_check: &[PathBuf],
    also_texts: &[String],
    budget: &ValidationBudget,
) -> Result<Vec<DiagnosticsReport>> {
    let hint = edits
        .first()
        .map(|(file, _)| file.as_path())
        .or_else(|| also_check.first().map(|p| p.as_path()));
    let unavailable = |err: &anyhow::Error| {
        edits
            .iter()
            .map(|(f, _)| f)
            .chain(also_check)
            .map(|file| incomplete_report(&display(root, file), &err.to_string()))
            .collect()
    };
    let mut baselines: HashMap<String, (DiagnosticsReport, String)> = HashMap::new();
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|&i| !crate::lang::is_header(&edits[i].0));
    // New nested modules must exist before their parent re-export is analyzed.
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
        std::cmp::Reverse(
            file.components()
                .count()
                .saturating_sub(usize::from(is_module_root)),
        )
    });
    let mut rust_order = rust_order.into_iter();
    for i in &mut order {
        if edits[*i].0.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            *i = rust_order
                .next()
                .expect("one Rust edit per Rust order slot");
        }
    }

    let mut before_symbols = Vec::with_capacity(order.len());
    let before_texts: Vec<_> = edits
        .iter()
        .map(|(file, text)| super::baseline::text(root, file, Some(text)))
        .chain(
            also_check
                .iter()
                .map(|file| super::baseline::text(root, file, None)),
        )
        .collect();
    {
        let checkout = budget
            .run(
                budget.session,
                "opening baseline session",
                LspSession::open_for_validation(remote, root, hint),
            )
            .await;
        let mut checkout = match checkout {
            Ok(session) => session,
            Err(err) => return Ok(unavailable(&err)),
        };
        // Install the entire before-state first: a baseline queried against another
        // file's already-written proposal would otherwise hide cross-file errors.
        let baseline_budget = budget.baseline();
        for (file, text) in edits
            .iter()
            .map(|(f, _)| f)
            .chain(also_check)
            .zip(&before_texts)
        {
            if let Err(err) = budget
                .run(
                    budget.auxiliary,
                    "opening baseline text",
                    checkout.open_text(file, text.as_deref().unwrap_or("")),
                )
                .await
            {
                return Ok(unavailable(&err));
            }
        }
        for (file, text) in edits
            .iter()
            .map(|(f, _)| f)
            .chain(also_check)
            .zip(&before_texts)
        {
            let Some(text) = text else {
                continue;
            };
            let uri = checkout.uri_for(file)?;
            // Baselines are optional evidence, and must not consume the proposal's budget.
            if let Ok(result) = baseline_budget
                .query(
                    &mut checkout,
                    file,
                    "textDocument/diagnostic",
                    serde_json::json!({"textDocument": {"uri": uri}}),
                    budget.auxiliary * 2,
                )
                .await
            {
                let shown = display(root, file);
                baselines.insert(shown.clone(), (parse_items(&shown, &result), text.clone()));
            }
        }
        for &i in &order {
            let file = &edits[i].0;
            let symbols = if before_texts[i].is_some() {
                let uri = checkout.uri_for(file)?;
                baseline_budget
                    .query(
                        &mut checkout,
                        file,
                        "textDocument/documentSymbol",
                        serde_json::json!({"textDocument": {"uri": uri}}),
                        budget.auxiliary,
                    )
                    .await
                    .map(|r| symbol_names(&r))
                    .unwrap_or_default()
            } else {
                BTreeSet::new()
            };
            before_symbols.push(symbols);
        }
        let _ = budget
            .run(
                Duration::from_millis(500),
                "closing baseline session",
                async {
                    checkout.close().await;
                    Ok(())
                },
            )
            .await;
    }

    let session = budget
        .run(
            budget.session,
            "opening proposal session",
            LspSession::open_for_validation(remote, root, hint),
        )
        .await;
    let mut session = match session {
        Ok(session) => session,
        Err(err) => return Ok(unavailable(&err)),
    };
    let mut sources = HashMap::new();
    let mut proposed = Vec::with_capacity(order.len());
    for &i in &order {
        let (file, text) = &edits[i];
        let uri = match budget
            .run(
                budget.auxiliary,
                "opening proposed text",
                session.open_text(file, text),
            )
            .await
        {
            Ok(uri) => uri,
            // A partial overlay is not a valid proposal. Drop the connection and fail every
            // file closed rather than checking callers against missing edits.
            Err(err) => return Ok(unavailable(&err)),
        };
        sources.insert(display(root, file), text.clone());
        proposed.push((i, file.clone(), uri));
    }
    // Opening also_check later can invalidate a snapshot of an edited file. Apply every
    // document write before starting any proposal query.
    for (file, text) in also_check.iter().zip(also_texts) {
        if !edits.iter().any(|(edited, _)| edited == file)
            && let Err(err) = budget
                .run(
                    budget.auxiliary,
                    "opening checked caller",
                    session.open_text(file, text),
                )
                .await
        {
            return Ok(unavailable(&err));
        }
        sources
            .entry(display(root, file))
            .or_insert_with(|| text.clone());
    }

    let mut missing = Vec::new();
    for ((_, file, uri), before) in proposed.iter().zip(before_symbols) {
        if let Ok(after) = budget
            .query(
                &mut session,
                file,
                "textDocument/documentSymbol",
                serde_json::json!({"textDocument": {"uri": uri}}),
                budget.auxiliary,
            )
            .await
        {
            for name in before.difference(&symbol_names(&after)) {
                missing.push((name.clone(), display(root, file)));
            }
        }
    }

    // Queries run in caller order; every failure becomes a file-specific unsuccessful
    // report. The same deadline covers edited files and unchanged callers.
    let mut reports = Vec::with_capacity(edits.len() + also_check.len());
    for file in edits.iter().map(|(f, _)| f).chain(also_check) {
        let uri = session.uri_for(file)?;
        let shown = display(root, file);
        let result = budget
            .query(
                &mut session,
                file,
                "textDocument/diagnostic",
                serde_json::json!({"textDocument": {"uri": uri}}),
                budget.diagnostic,
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                reports.push(match platform_exclusion_reason(file, &err) {
                    Some(reason) => platform_excluded_report(&shown, &reason),
                    None => incomplete_report(&shown, &err.to_string()),
                });
                budget.completed(1);
                continue;
            }
        };
        let mut report = parse_items(&shown, &result);
        if let Some(text) = sources.get(&shown) {
            if let Some((before, before_text)) = baselines.get(&shown) {
                set_aside_preexisting(&mut report, text, before, before_text);
            }
            set_aside_derive_expansions(&mut report, text);
        }
        refuse_unchecked(&mut report);
        reports.push(report);
        budget.completed(1);
    }
    let _ = budget
        .run(budget.auxiliary, "checking re-exports", async {
            suppress_used_public_reexport_warnings(
                &mut session,
                root,
                edits.len(),
                &mut reports,
                &sources,
            )
            .await;
            Ok(())
        })
        .await;
    // What the definition check found before its deadline or a failed request still counts,
    // and the tokens it did not reach are warned about as unresolved, with a note that the
    // check was cut short (#1465).
    let mut resolved = BTreeSet::new();
    let complete = budget
        .run(budget.auxiliary, "resolving moved symbols", async {
            Ok(resolved_tokens(
                &mut session,
                root,
                &reports,
                &sources,
                &missing,
                &mut resolved,
            )
            .await)
        })
        .await
        .unwrap_or(false);
    annotate_missing_symbols(&mut reports, &sources, &missing, &resolved);
    if !complete {
        crate::diagnostics::annotate::mark_unfinished_resolution(&mut reports);
    }
    let _ = budget
        .run(
            Duration::from_millis(500),
            "closing proposal session",
            async {
                session.close().await;
                Ok(())
            },
        )
        .await;
    let _ = budget
        .run(budget.auxiliary, "checking Swift target", async {
            reconcile_swift_cross_target_diagnostics(remote, root, edits, &mut reports).await
        })
        .await;
    Ok(reports)
}
