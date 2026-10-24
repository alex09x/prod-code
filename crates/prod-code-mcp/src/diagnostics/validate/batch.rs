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
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use super::budget::{ValidationBudget, incomplete_report};
use super::engine::validate_texts_single_engine;
use super::progress::ValidationProgress;
use crate::diagnostics::reexport::display;
use crate::diagnostics::syntax::{
    ensure_source_file, is_parser_validated_file, validate_file_content,
};
use crate::diagnostics::types::DiagnosticsReport;

/// Validates a complete proposal in a private overlay. A file whose diagnostics are
/// unavailable gets an unsuccessful report without discarding the rest of the batch.
/// Nothing is written anywhere.
pub async fn validate_texts(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
) -> Result<Vec<DiagnosticsReport>> {
    let reports = Box::pin(validate_texts_with_budget(
        remote,
        root,
        edits,
        also_check,
        &ValidationBudget::new(),
    ))
    .await?;
    // A refactoring decides whether to write from this: a file that was not checked is not a
    // compile error, and must read as "could not be validated", as before partial results.
    if let Some(item) = reports
        .iter()
        .flat_map(|r| r.items.iter())
        .find(|d| d.code.as_deref() == Some("prod-code-validation-incomplete"))
    {
        anyhow::bail!("{}", item.message);
    }
    Ok(reports)
}

/// `validate_texts` for an agent's own `code_validate_edits` call: it answers before the MCP
/// client's tool timeout, naming the files it could not check in time.
pub async fn validate_texts_interactive(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
) -> Result<Vec<DiagnosticsReport>> {
    Box::pin(validate_texts_with_budget(
        remote,
        root,
        edits,
        also_check,
        &ValidationBudget::interactive(),
    ))
    .await
}

/// The interactive batch deadline and partial reports, with phase and completion
/// snapshots for a CLI progress reporter. Refactoring validation keeps its own budget.
pub async fn validate_texts_interactive_with_progress(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
    progress: ValidationProgress,
) -> Result<Vec<DiagnosticsReport>> {
    let budget = ValidationBudget {
        progress: Some(progress),
        ..ValidationBudget::interactive()
    };
    Box::pin(validate_texts_with_budget(
        remote, root, edits, also_check, &budget,
    ))
    .await
}

pub(super) async fn validate_texts_with_budget(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
    budget: &ValidationBudget,
) -> Result<Vec<DiagnosticsReport>> {
    for file in edits.iter().map(|(file, _)| file).chain(also_check) {
        if !is_parser_validated_file(file) {
            ensure_source_file(file)?;
        }
    }
    // Unreadable also_check files must never be silently treated as clean (#446).
    let mut also_texts = Vec::with_capacity(also_check.len());
    for file in also_check {
        let abs = if file.is_absolute() {
            file.clone()
        } else {
            root.join(file)
        };
        also_texts.push(std::fs::read_to_string(&abs).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}, which the change must be checked against; nothing was validated: {e}",
                abs.display()
            )
        })?);
    }
    if edits.iter().all(|(f, _)| is_parser_validated_file(f))
        && also_check.iter().all(|f| is_parser_validated_file(f))
    {
        budget.completed(edits.len() + also_check.len());
        return Ok(edits
            .iter()
            .map(|(file, text)| (file, text))
            .chain(also_check.iter().zip(&also_texts))
            .map(|(file, text)| validate_file_content(&display(root, file), file, text))
            .collect());
    }

    let engines: HashSet<&'static str> = edits
        .iter()
        .map(|(p, _)| p)
        .chain(also_check)
        .map(|p| crate::lang::engine_group_for_path(p))
        .collect();
    if engines.len() == 1 {
        return validate_texts_single_engine(remote, root, edits, also_check, &also_texts, budget)
            .await;
    }
    let mut final_reports = vec![None; edits.len() + also_check.len()];
    for engine in engines {
        if matches!(engine, "json" | "markdown" | "xml") {
            for (i, (file, text)) in edits.iter().enumerate() {
                if crate::lang::engine_group_for_path(file) == engine {
                    final_reports[i] =
                        Some(validate_file_content(&display(root, file), file, text));
                    budget.completed(1);
                }
            }
            for (i, (file, text)) in also_check.iter().zip(&also_texts).enumerate() {
                if crate::lang::engine_group_for_path(file) == engine {
                    final_reports[edits.len() + i] =
                        Some(validate_file_content(&display(root, file), file, text));
                    budget.completed(1);
                }
            }
            continue;
        }
        let edit_indices: Vec<_> = edits
            .iter()
            .enumerate()
            .filter(|(_, (file, _))| crate::lang::engine_group_for_path(file) == engine)
            .map(|(i, _)| i)
            .collect();
        let also_indices: Vec<_> = also_check
            .iter()
            .enumerate()
            .filter(|(_, file)| crate::lang::engine_group_for_path(file) == engine)
            .map(|(i, _)| i)
            .collect();
        let group_edits: Vec<_> = edit_indices.iter().map(|&i| edits[i].clone()).collect();
        let group_also: Vec<_> = also_indices
            .iter()
            .map(|&i| also_check[i].clone())
            .collect();
        let group_texts: Vec<_> = also_indices
            .iter()
            .map(|&i| also_texts[i].clone())
            .collect();
        let sub_reports = validate_texts_single_engine(
            remote,
            root,
            &group_edits,
            &group_also,
            &group_texts,
            budget,
        )
        .await?;
        for (i, report) in edit_indices
            .into_iter()
            .chain(also_indices.into_iter().map(|i| edits.len() + i))
            .zip(sub_reports)
        {
            final_reports[i] = Some(report);
        }
    }
    Ok(final_reports
        .into_iter()
        .enumerate()
        .map(|(i, report)| {
            report.unwrap_or_else(|| {
                let file = if i < edits.len() {
                    &edits[i].0
                } else {
                    &also_check[i - edits.len()]
                };
                incomplete_report(&display(root, file), "no matching analyzer result")
            })
        })
        .collect())
}
