/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Boundaries of a Go result replacement that its direct-call evidence and its one-module
//! compile cannot see (#1411): a result written in parentheses, callers in another module of a
//! `go.work`, and a method that can be called by its name.

use crate::signature_go::text::{display, skip_space};
use crate::signature_go::types::{Call, Decl, refusal};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The one unnamed result of `results` without the parentheses it may be written in:
/// `(int)` is `int`. Named or several results keep their spelling, and are refused later.
pub(crate) fn bare_result(results: &str) -> &str {
    results
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .map(str::trim)
        .filter(|inner| !inner.is_empty() && !inner.contains([',', ' ', '\t', '(']))
        .unwrap_or(results)
}

/// Where the result token `bare` starts in `text`, looking from just after the parameter list
/// at `close`: at the token itself, or inside the one pair of parentheses around it.
pub(crate) fn result_token_at(text: &str, close: usize, bare: &str) -> Option<usize> {
    let at = skip_space(text, close + 1);
    if text[at..].starts_with(bare) {
        return Some(at);
    }
    let inner = skip_space(text, text[at..].strip_prefix('(').map(|_| at + 1)?);
    let after = skip_space(text, inner + bare.len());
    (text[inner..].starts_with(bare) && text[after..].starts_with(')')).then_some(inner)
}

/// The directory of the `go.mod` that owns `source`, inside `root`.
fn module_of(root: &Path, source: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let source = std::fs::canonicalize(source).ok()?;
    source
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&root))
        .find(|dir| dir.join("go.mod").is_file())
        .map(Path::to_path_buf)
}

/// Refuses a call in a Go module other than the declaration's. Only the declaring module is
/// compiled with the change, so a sibling module of a `go.work` that uses the result would go
/// unchecked.
pub(crate) fn refuse_other_module_callers(
    root: &Path,
    file: &Path,
    decl: &Decl,
    calls: &[Call],
) -> Result<()> {
    let module = module_of(root, file);
    for call in calls {
        let theirs = module_of(root, &call.path);
        anyhow::ensure!(
            theirs == module,
            "{}",
            refusal(format!(
                "{}: this caller of `{}` is in another Go module ({}), and only the declaring \
                 module is compiled with the change, so the replacement cannot be verified there",
                call.at,
                decl.name,
                theirs.map_or_else(
                    || "unknown".to_string(),
                    |m| display(root, &m.join("go.mod"))
                )
            ))
        );
    }
    Ok(())
}

/// Refuses an exported method that code can call by its name, where no reference shows it.
/// That is possible when a Go file of the checkout imports `reflect` or a template package, and
/// in any package other than `main`, which code outside the checkout can import.
pub(crate) fn refuse_name_based_calls(root: &Path, text: &str, decl: &Decl) -> Result<()> {
    if decl.receiver.is_none() || !decl.name.starts_with(|c: char| c.is_ascii_uppercase()) {
        return Ok(());
    }
    let package = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("package "))
        .map(str::trim);
    anyhow::ensure!(
        package == Some("main"),
        "{}",
        refusal(format!(
            "changing the result of exported method `{}` is refused: package `{}` can be \
             imported by code outside the checkout, which can call the method by its name \
             through reflection",
            decl.name,
            package.unwrap_or("?")
        ))
    );
    let by_name = ["\"reflect\"", "\"text/template\"", "\"html/template\""];
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "go") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(import) = by_name.iter().find(|i| source.contains(*i)) {
            anyhow::bail!(
                "{}",
                refusal(format!(
                    "changing the result of exported method `{}` is refused: {} imports {import}, \
                     which can call a method by its name with no reference to it",
                    decl.name,
                    display(root, path)
                ))
            );
        }
    }
    Ok(())
}
