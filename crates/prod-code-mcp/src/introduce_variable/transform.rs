/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::analysis::{
    can_panic, changes, enclosing_body, innermost_block, loops_after, occurrences, parenthesised,
    reads_and_effects, statement_start, surely_evaluated,
};
use super::types::{Introduced, is_ident};

/// Introduces `name` for the expression selected at `line`:`col` .. `end_line`:`end_col` of
/// `file`, replacing every occurrence of it in the enclosing function.
#[allow(clippy::too_many_arguments)]
pub async fn introduce_variable(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    apply: bool,
    force: bool,
) -> Result<Introduced> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");
    let expr = text[start..end].trim().to_string();
    anyhow::ensure!(!expr.contains(';'), "the selection is not one expression");
    let (names, effect) = reads_and_effects(&expr);
    if let Some(effect) = effect {
        anyhow::bail!(
            "`{expr}` cannot be evaluated once for every place it occurs: {effect}. Use \
             `code_assist` with `extract_variable` for this one occurrence"
        );
    }

    // The innermost function whose body holds the selection.
    let (body_open, body_close) =
        enclosing_body(&text, start).context("the selection is not inside a function")?;
    let found = occurrences(&text, body_open, body_close, &expr);
    let first = *found
        .first()
        .context("the selection is not in the function")?;
    let last = *found.last().unwrap_or(&first);
    let anchor = statement_start(&text, innermost_block(&text, body_open, first, last), first);
    // A loop that starts after the binding and holds a later occurrence runs that occurrence
    // again, after whatever the rest of its body changed.
    let mut watched = vec![(anchor, last + expr.len())];
    watched.extend(
        loops_after(&text, anchor, last)
            .into_iter()
            .filter(|(open, close)| found.iter().any(|at| open < at && at < close)),
    );
    for n in &names {
        anyhow::ensure!(
            !watched
                .iter()
                .any(|(from, to)| changes(&text[*from..*to], n)),
            "`{n}` changes between the first occurrence of `{expr}` and the last, so one value \
             cannot stand for all of them"
        );
    }
    // Binding it earlier evaluates it where the code may not have: a division under
    // `if b != 0` would divide by zero. That is the same program only if one occurrence
    // runs whenever the binding does.
    if can_panic(&expr) {
        let anywhere = found.iter().any(|at| {
            let statement =
                statement_start(&text, innermost_block(&text, body_open, *at, *at), *at);
            surely_evaluated(&text, anchor, statement, *at)
        });
        anyhow::ensure!(
            anywhere,
            "`{expr}` can panic, and no occurrence of it runs every time the binding would \
             (each is under a condition, or after a way out). Use `code_assist` with \
             `extract_variable` for one occurrence"
        );
    }

    // `let name = expr;` above the statement that holds the first occurrence, in the innermost
    // block that holds them all, at its indentation.
    let stmt_line_start = text[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[stmt_line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let bare = expr
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
        .filter(|inner| {
            crate::parameter_object::matching_bracket(&expr, 0) == Some(expr.len() - 1)
                && !inner.is_empty()
        })
        .unwrap_or(&expr);
    let mut new_text = text.clone();
    for at in found.iter().rev() {
        new_text.replace_range(parenthesised(&text, *at, *at + expr.len()), name);
    }
    new_text.insert_str(stmt_line_start, &format!("{indent}let {name} = {bare};\n"));

    let rewritten: BTreeMap<PathBuf, String> =
        std::iter::once((file.to_path_buf(), new_text.clone())).collect();
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &[],
    )
    .await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
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
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(Introduced {
        name: name.to_string(),
        expression: bare.to_string(),
        root: root.to_path_buf(),
        file: file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned(),
        occurrences: found.len(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
