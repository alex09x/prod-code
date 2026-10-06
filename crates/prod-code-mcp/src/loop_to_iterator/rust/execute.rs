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
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::chain::chain;
use super::recognise::recognise;
use crate::loop_to_iterator::helpers::{binding_type, is_ident};
use crate::loop_to_iterator::types::{Rewritten, Shape};

/// The analyzer's type for the binding whose name starts at byte `at` of `text`.
async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
) -> Option<String> {
    let (line, col) = crate::signature::line_col_at(text, at)?;
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) }
        }),
    )
    .await
    .ok()?;
    binding_type(hover.pointer("/contents/value")?.as_str()?)
}

/// Rewrites the accumulator loop whose `for` is at `line`:`col` of `file`.
pub async fn loop_to_iterator(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<Rewritten> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at =
        crate::signature::offset_of(&text, line, col).context("the position is not in the file")?;
    let l = recognise(&text, at)?;

    // The type: declared, else the analyzer's for the accumulator; a vector may stay `Vec<_>`.
    let ty = match (&l.declared, &l.shape) {
        (Some(t), _) => t.clone(),
        (None, Shape::Collect { .. }) => "Vec<_>".to_string(),
        (None, Shape::Find { .. }) => "Option<_>".to_string(),
        (None, Shape::Any { .. } | Shape::All { .. }) => "bool".to_string(),
        (None, _) => {
            let name_at = l.start + text[l.start..].find(&l.acc).unwrap_or(0);
            hover_type(remote, root, file, &text, name_at)
                .await
                .with_context(|| format!("the analyzer gives no type for `{}`", l.acc))?
        }
    };
    // What the loop iterates, when it is a name: a reference is iterated with `iter()`.
    let source_at = text[l.start..l.end]
        .find(&format!(" in {}", l.source))
        .map(|i| l.start + i + 4);
    let source_type = match source_at {
        Some(at) if l.source.chars().all(|c| is_ident(c) || c == '.') => {
            let last = at + l.source.rfind('.').map_or(0, |i| i + 1);
            hover_type(remote, root, file, &text, last).await
        }
        _ => None,
    };
    if let Shape::Count { .. } = l.shape {
        anyhow::ensure!(
            ty == "usize",
            "`{}` is a `{ty}`, and `count()` gives a `usize`",
            l.acc
        );
    }
    if matches!(l.shape, Shape::Any { .. } | Shape::All { .. }) {
        anyhow::ensure!(
            ty == "bool",
            "`{}` is a `{ty}`, and predicate tests give a `bool`",
            l.acc
        );
    }
    if matches!(l.shape, Shape::Find { .. }) {
        anyhow::ensure!(
            ty.starts_with("Option<") || ty == "Option<_>",
            "`{}` is a `{ty}`, and find gives an `Option`",
            l.acc
        );
    }

    // Without `mut` first; the analyzer says when the variable is still changed afterwards.
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();
    let mut outcome = None;
    for mutable in [false, true] {
        let statement = chain(&l, &ty, source_type.as_deref(), mutable);
        let mut new_text = text.clone();
        new_text.replace_range(l.start..l.end, &statement);
        let reports = crate::diagnostics::validate_texts(
            remote,
            root,
            &[(file.to_path_buf(), new_text.clone())],
            &[],
        )
        .await?;
        let errors: Vec<&crate::diagnostics::DocDiagnostic> = reports
            .iter()
            .flat_map(|r| r.items.iter())
            .filter(|d| d.severity == "error")
            .collect();
        let needs_mut = errors
            .iter()
            .any(|d| matches!(d.code.as_deref(), Some("need-mut" | "E0596" | "E0384")));
        let diagnostics: Vec<String> = errors
            .iter()
            .map(|d| {
                format!(
                    "{}{} ({rel}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    d.line,
                    d.col
                )
            })
            .collect();
        outcome = Some((statement, new_text, diagnostics));
        if !needs_mut {
            break;
        }
    }
    let (statement, new_text, diagnostics) = outcome.context("no attempt was made")?;
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Rewritten {
        statement,
        root: root.to_path_buf(),
        file: rel,
        new_text,
        diagnostics,
        applied,
    })
}
