/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod callers;
mod enclosing;
mod find;
mod import;
mod restructure;
mod returns;
mod shadow;

pub use enclosing::enclosing_polyglot_info;
pub use find::find_polyglot_decl;
pub use restructure::restructure_declaring_file;

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::Language;
use crate::wrap_return::rust::wrap_rust_ext;
use crate::wrap_return::types::{WrappedReturn, Wrapper};
use crate::wrap_return::utils::{display, one_based_lsp_position};
use callers::collect_and_rewrite_callers;

/// Unified wrap_return refactoring across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    wrap_polyglot_ext(
        remote, root, file, symbol, line, col, wrapper, None, error, apply, force,
    )
    .await
}

/// Unified wrap_return refactoring with optional custom constructor across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_polyglot_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    wrapper: Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    let lang = Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return wrap_rust_ext(
            remote,
            root,
            file,
            symbol,
            line.unwrap_or(0),
            col.unwrap_or(0),
            wrapper,
            constructor,
            error,
            apply,
            force,
        )
        .await;
    }
    if lang == Language::Java {
        anyhow::bail!("wrap_return does not support Java yet");
    }

    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_decl(&text, lang, symbol, line)?;
    let name = decl.name.clone();
    let (selected_line, selected_col) = one_based_lsp_position(&text, decl.name_start);
    let mut semantic_references =
        crate::signature::references(remote, root, file, selected_line, selected_col)
            .await?
            .into_iter()
            .map(|(path, ref_line, ref_col)| {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                (path, ref_line, ref_col)
            })
            .collect::<HashSet<_>>();
    let was = decl.was.clone();

    // Check already wrapped
    match &wrapper {
        Wrapper::Promise => {
            anyhow::ensure!(
                !was.contains("Promise<") && (!decl.is_async || !was.is_empty()),
                "`{name}` already returns a `Promise`"
            );
        }
        Wrapper::Option => {
            anyhow::ensure!(
                !was.contains("Optional[")
                    && !was.contains("std::optional")
                    && !was.contains("optional")
                    && !was.ends_with('?')
                    && !was.contains("| null")
                    && !was.contains("| None")
                    && !was.starts_with('*')
                    && !was.starts_with("Option<"),
                "`{name}` already returns an `Option`"
            );
        }
        Wrapper::Result => {
            anyhow::ensure!(
                !was.contains("Result<")
                    && !was.contains("Result[")
                    && !was.contains("std::expected")
                    && !was.contains("expected")
                    && !was.contains("error")
                    && !was.contains("{ ok:"),
                "`{name}` already returns a `Result`"
            );
        }
        Wrapper::Pointer => {
            anyhow::ensure!(
                !was.starts_with('*'),
                "`{name}` already returns a `Pointer`"
            );
        }
        Wrapper::Custom(custom_name) => {
            let base = custom_name
                .split(['<', '['])
                .next()
                .unwrap_or(custom_name)
                .trim();
            let base = base.rsplit("::").next().unwrap_or(base);
            let base = base.rsplit('.').next().unwrap_or(base).trim();
            anyhow::ensure!(!was.contains(base), "`{name}` already returns a `{base}`");
        }
    }

    let (new_decl_file, now) =
        restructure_declaring_file(&text, lang, &decl, &wrapper, constructor, error)?;

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_decl_file.clone());

    let mut propagated = 0usize;
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();

    collect_and_rewrite_callers(
        root,
        file,
        &text,
        lang,
        &decl,
        &wrapper,
        &now,
        &was,
        constructor,
        error,
        &mut semantic_references,
        &mut rewritten,
        &mut propagated,
        &mut blocked,
        &mut unmatched,
    )?;

    for (path, ref_line, ref_col) in semantic_references {
        unmatched.push(format!(
            "{}:{ref_line}:{ref_col}: analyzer reference to `{name}` could not be rewritten safely",
            display(root, &path)
        ));
    }

    rewritten.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
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
            blocked.is_empty() || force,
            "{} call site(s) cannot propagate; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(WrappedReturn {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        propagated,
        blocked,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
