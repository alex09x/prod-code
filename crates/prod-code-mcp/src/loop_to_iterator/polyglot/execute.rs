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

use super::cpp::recognise_cpp;
use super::go::recognise_go;
use super::python::recognise_python;
use super::swift::recognise_swift;
use super::ts::recognise_ts;
use crate::loop_to_iterator::helpers::find_loop_offset;
use crate::loop_to_iterator::rust::execute::loop_to_iterator;
use crate::loop_to_iterator::types::Rewritten;
use crate::parameter_object::Language;

/// Polyglot entry point for converting an accumulating loop into an iterator chain or functional expression.
#[allow(clippy::too_many_arguments)]
pub async fn loop_to_iterator_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    apply: bool,
    force: bool,
) -> Result<Rewritten> {
    let lang = crate::parameter_object::Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;

    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();

    if lang == Language::Rust {
        let (l, c) = match (line, col) {
            (Some(l), Some(c)) => (l, c),
            (Some(l), None) => (l, 1),
            _ => {
                let offset = find_loop_offset(&text, lang, symbol, line, col)?;
                crate::signature::line_col_at(&text, offset).with_context(|| {
                    format!("cannot find line/col for loop in {}", file.display())
                })?
            }
        };
        return loop_to_iterator(remote, root, file, l, c, apply, force).await;
    }
    if lang == Language::Java {
        anyhow::bail!("loop_to_iterator does not support Java yet");
    }

    let offset = find_loop_offset(&text, lang, symbol, line, col)?;

    let polyglot_loop = match lang {
        Language::TypeScript | Language::JavaScript => recognise_ts(&text, offset)?,
        Language::Python => recognise_python(&text, offset)?,
        Language::Swift => recognise_swift(&text, offset)?,
        Language::Cpp | Language::C => recognise_cpp(&text, offset)?,
        Language::Go => recognise_go(&text, offset)?,
        Language::Rust | Language::Java => unreachable!(),
    };

    let mut new_text = text.clone();
    new_text.replace_range(
        polyglot_loop.start..polyglot_loop.end,
        &polyglot_loop.replacement,
    );

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
        statement: polyglot_loop.statement,
        root: root.to_path_buf(),
        file: rel,
        new_text,
        diagnostics,
        applied,
    })
}
