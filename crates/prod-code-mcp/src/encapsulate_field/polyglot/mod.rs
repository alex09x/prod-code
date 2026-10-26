/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cpp;
mod cpp_statement;
mod expressions;
pub mod go;
pub mod lsp;
pub mod py;
pub(super) mod python_rhs;
pub mod replace;
mod semantic;
pub mod swift;
pub mod ts_js;

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::case::{display, is_ident};
use super::types::{EncapsulatedField, Language};
pub use cpp::{encapsulate_field_cpp, rewrite_external_cpp};
pub use go::{encapsulate_field_go, rewrite_external_go};
pub(crate) use lsp::field_position_in_lsp;
pub use lsp::{field_position_in_symbols, lsp_symbol_position};
pub use py::{encapsulate_field_py, rewrite_external_py};
pub use replace::{
    replace_cpp_unqualified, replace_line_self, replace_line_this, replace_line_this_private,
};
pub use swift::{encapsulate_field_swift, rewrite_external_swift};
pub use ts_js::{encapsulate_field_js, encapsulate_field_ts, rewrite_external_ts};

#[cfg(test)]
fn has_ambiguous_property_use(text: &str, field: &str) -> bool {
    [format!(".{field}"), format!("->{field}")]
        .iter()
        .any(|needle| {
            text.match_indices(needle).any(|(at, _)| {
                let before = text[..at].trim_end();
                let receiver = before
                    .rsplit(|c: char| !is_ident(c))
                    .next()
                    .unwrap_or_default();
                receiver != "this" && receiver != "self"
            })
        })
}

#[cfg(test)]
fn rewrite_external_line(lang: Language, line: &str, field: &str) -> (String, usize, usize) {
    match lang {
        Language::TypeScript | Language::JavaScript => rewrite_external_ts(line, field),
        Language::Python => rewrite_external_py(line, field),
        Language::Cpp => rewrite_external_cpp(line, field),
        Language::Swift => rewrite_external_swift(line, field),
        Language::Go => rewrite_external_go(line, field),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn encapsulate_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    field_name: &str,
    by_value: Option<bool>,
    apply: bool,
    force: bool,
) -> Result<EncapsulatedField> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path).context("unsupported encapsulation language")?;
    // Only metadata is taken from this draft. Generated text is discarded until semantic
    // discovery supplies the declaration and the selected references.
    let (owner, ty, _, _, _, _) =
        semantic::generate(lang, &content, class_name, field_name, by_value)?;
    let canonical_root = std::fs::canonicalize(workspace_root)?;
    let canonical_file = std::fs::canonicalize(file_path)?;
    let mut unmatched = Vec::new();
    let mut references_by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
    let mut declaration = None;
    match field_position_in_lsp(remote, workspace_root, file_path, &owner, field_name).await {
        Ok(Some((line, col))) => {
            declaration = crate::signature::offset_of(&content, line, col).filter(|at| {
                content[*at..].starts_with(field_name)
                    && (*at == 0 || !content[..*at].ends_with(is_ident))
                    && !content[*at + field_name.len()..].starts_with(is_ident)
            });
            if declaration.is_none() {
                unmatched.push("field symbol does not point at the selected declaration".into());
            }
            match crate::signature::references(remote, workspace_root, file_path, line, col).await {
                Ok(refs) => {
                    for (path, line, col) in refs {
                        let path = std::fs::canonicalize(&path).unwrap_or(path);
                        references_by_file
                            .entry(path)
                            .or_default()
                            .push((line, col));
                    }
                }
                Err(err) => unmatched.push(format!(
                    "analyzer references could not be verified: {err:#}"
                )),
            }
        }
        Ok(None) => unmatched.push("field symbols could not be resolved".into()),
        Err(err) => unmatched.push(format!("field symbols could not be resolved: {err:#}")),
    }
    let (mut reads, mut writes, mut left_in_file) = (0, 0, 0);
    let mut rewritten = Vec::new();
    if let Some(decl) = declaration {
        references_by_file
            .entry(canonical_file.clone())
            .or_default();
        for (path, positions) in &references_by_file {
            if !path.starts_with(&canonical_root) || !lang.matches_extension(path) {
                unmatched.push(format!(
                    "{}: analyzer reference is outside the supported workspace language",
                    display(workspace_root, path)
                ));
                continue;
            }
            let original = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(err) => {
                    unmatched.push(format!(
                        "{}: referenced source could not be read: {err}",
                        display(workspace_root, path)
                    ));
                    continue;
                }
            };
            let own = (path == &canonical_file).then_some((decl, owner.as_str()));
            let (changed, r, w, i, failures) =
                semantic::rewrite(&original, path, lang, field_name, positions, own, by_value)?;
            unmatched.extend(
                failures
                    .into_iter()
                    .map(|why| format!("{}:{why}", display(workspace_root, path))),
            );
            reads += r;
            writes += w;
            left_in_file += i;
            if changed != original {
                rewritten.push((path.to_string_lossy().into_owned(), changed));
            }
        }
    }
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports =
        crate::diagnostics::validate_texts(remote, workspace_root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.source
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                d.message,
                d.line,
                d.col
            )
        })
        .collect();
    let mut applied = false;
    if apply && unmatched.is_empty() && (diagnostics.is_empty() || force) {
        let map = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        crate::refactor::apply_workspace_edit(
            workspace_root,
            &crate::signature::whole_file_edit(&map),
        )?;
        applied = true;
    }
    Ok(EncapsulatedField {
        owner,
        root: workspace_root.to_path_buf(),
        file: display(workspace_root, file_path),
        field: field_name.into(),
        ty,
        by_value: by_value.unwrap_or(false),
        reads,
        writes,
        chained_reads: 0,
        left_in_file,
        blocked: vec![],
        unmatched,
        rewritten,
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests;
