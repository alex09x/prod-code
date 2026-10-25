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
        name.starts_with(|c: char| c == '_' || unicode_ident::is_xid_start(c))
            && name.chars().all(is_ident)
            && ![
                "_", "self", "Self", "super", "crate", "let", "fn", "for", "in", "match", "if",
                "else", "loop", "while", "return", "break", "continue", "as", "const", "static",
                "mut", "ref", "move", "async", "await", "dyn", "impl", "trait", "type", "struct",
                "enum", "union", "pub", "use", "mod", "where", "unsafe", "extern", "true", "false",
                "try", "yield", "gen", "abstract", "become", "box", "do", "final", "macro",
                "override", "priv", "typeof", "unsized", "virtual"
            ]
            .contains(&name),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");
    let selected = &text[start..end];
    let start = start + selected.len() - selected.trim_start().len();
    let end = end - (selected.len() - selected.trim_end().len());
    anyhow::ensure!(start < end, "the selection is empty");
    let plan = super::planning::plan(remote, root, file, &text, start..end, name).await?;
    let anchor = plan.anchor;

    // `let name = expr;` above the statement that holds the first occurrence, in the innermost
    // block that holds them all, at its indentation.
    let stmt_line_start = text[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[stmt_line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let bare = &text[plan.initializer];
    let mut new_text = text.clone();
    for range in plan.replacements.iter().rev() {
        new_text.replace_range(range.clone(), name);
    }
    if text[stmt_line_start..anchor]
        .chars()
        .all(char::is_whitespace)
    {
        new_text.insert_str(stmt_line_start, &format!("{indent}let {name} = {bare};\n"));
    } else {
        // A statement can begin after another statement or an opening brace on the same
        // line. Inserting at the line start would move it outside its lexical scope.
        new_text.insert_str(anchor, &format!("let {name} = {bare}; "));
    }

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
            std::fs::read_to_string(file)? == text,
            "the source changed during planning; nothing was written"
        );
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
        occurrences: plan.replacements.len(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
