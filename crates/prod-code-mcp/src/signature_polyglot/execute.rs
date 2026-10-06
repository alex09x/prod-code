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
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::{Language, call_args_span};
use crate::signature::{Modifiers, Param, SignatureChange};

use super::call_sites::rewrite_file_calls;
use super::declaration::{build_decl_edits, find_polyglot_declaration, format_polyglot_param};
use super::sources::collect_workspace_sources;
use super::syntax::{display, is_ident, is_import_or_export_context, is_in_comment, is_word_used};

#[allow(clippy::too_many_arguments)]
pub async fn change_with(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    let lang = Language::of(file).context("unsupported language for polyglot change_signature")?;
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;

    let decl = find_polyglot_declaration(&text, lang, line, col)?;
    let old_signature = text[decl.open_paren + 1..decl.close_paren]
        .trim()
        .to_string();
    let sources = collect_workspace_sources(root, lang);
    let has_call_candidates = sources.iter().any(|src_path| {
        let content = if src_path == file {
            text.clone()
        } else if let Ok(content) = std::fs::read_to_string(src_path) {
            content
        } else {
            return false;
        };
        content.match_indices(&decl.fn_name).any(|(at, _)| {
            let after = &content[at + decl.fn_name.len()..];
            !(at > 0 && content[..at].chars().next_back().is_some_and(is_ident))
                && !after.starts_with(is_ident)
                && !is_in_comment(&content, at, lang)
                && !crate::inline_parameter::is_in_string(&content, at, lang)
                && !is_import_or_export_context(&content, at, lang)
                && !(src_path == file
                    && at >= decl.open_paren.saturating_sub(decl.fn_name.len() + 20)
                    && at <= decl.close_paren)
                && call_args_span(&content, at + decl.fn_name.len()).is_some()
        })
    });
    let mut semantic_references = if has_call_candidates {
        crate::signature::references(remote, root, file, line, col)
            .await?
            .into_iter()
            .map(|(path, ref_line, ref_col)| {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                (path, ref_line, ref_col)
            })
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };

    // Safety check 1: verify all Keep params exist in declaration
    for r in request {
        if let Param::Keep(name) = r
            && !decl.params.iter().any(|d| &d.name == name)
        {
            anyhow::bail!("`{name}` is not a declared parameter");
        }
    }

    // Safety check 2: duplicate parameter names in request
    let mut seen = HashSet::new();
    for r in request {
        let name = match r {
            Param::Keep(n) => n,
            Param::Add { name, .. } => name,
        };
        if !seen.insert(name) {
            anyhow::bail!("duplicate parameter `{name}`");
        }
    }

    // Safety check 3: dropped parameters must not be used in the body unless force
    let mut dropped = Vec::new();
    for d in &decl.params {
        if !request.iter().any(|r| match r {
            Param::Keep(name) => name == &d.name,
            Param::Add { name, .. } => name == &d.name,
        }) {
            dropped.push(d.name.clone());
        }
    }

    if !dropped.is_empty() && !force {
        let body_text = &text[decl.body_open..decl.body_close];
        for gone in &dropped {
            if is_word_used(body_text, gone) {
                anyhow::bail!(
                    "parameter `{gone}` is still used in the function body; pass force: true to override"
                );
            }
        }
    }

    // Build the new declaration parameter list
    let mut new_decl_params = Vec::new();
    if let Some(r) = &decl.receiver {
        new_decl_params.push(r.clone());
    }

    for r in request {
        match r {
            Param::Keep(name) => {
                let orig = decl.params.iter().find(|d| &d.name == name).unwrap();
                new_decl_params.push(orig.raw.trim().to_string());
            }
            Param::Add { name, ty, value } => {
                new_decl_params.push(format_polyglot_param(name, ty, value, lang));
            }
        }
    }

    let new_signature = new_decl_params.join(", ");
    let decl_edits = build_decl_edits(&text, &decl, &new_signature, modifiers, lang);

    // Apply call site edits across the workspace
    let mut rewritten_files = Vec::new();
    let mut unmatched = Vec::new();
    for src_path in &sources {
        let is_decl_file = src_path == file;
        let content = if is_decl_file {
            text.clone()
        } else {
            let Ok(c) = std::fs::read_to_string(src_path) else {
                continue;
            };
            c
        };

        if let Some(new_content) = rewrite_file_calls(
            root,
            src_path,
            &content,
            is_decl_file,
            &decl,
            &old_signature,
            &new_signature,
            &decl_edits,
            request,
            modifiers,
            lang,
            &mut semantic_references,
            &mut unmatched,
        ) {
            let rel_path = display(root, src_path);
            rewritten_files.push((rel_path, new_content));
        }
    }

    for (path, ref_line, ref_col) in semantic_references {
        unmatched.push(format!(
            "{}:{ref_line}:{ref_col}: analyzer reference to `{}` could not be rewritten safely",
            display(root, &path),
            decl.fn_name
        ));
    }

    // Format rule description
    let rule = format!(
        "{}({old_signature}) -> {}({new_signature})",
        decl.fn_name, decl.fn_name
    );

    // In-memory overlay validation
    let files_to_validate: Vec<(PathBuf, String)> = rewritten_files
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &files_to_validate, &[]).await?;
    let overlay_diags: Vec<String> = reports
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

    // If apply is true and no diagnostics, write all files
    if apply {
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} call site(s) could not be rewritten safely; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            overlay_diags.is_empty() || force,
            "the analyzer reports {} error(s); nothing was written:\n  {}",
            overlay_diags.len(),
            overlay_diags.join("\n  ")
        );
        for (rel, content) in &rewritten_files {
            let full_path = root.join(rel);
            std::fs::write(&full_path, content)
                .with_context(|| format!("cannot write {}", full_path.display()))?;
        }
    }

    Ok(SignatureChange {
        symbol: decl.fn_name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature,
        new_signature,
        rule,
        rewritten: rewritten_files,
        unmatched,
        unexpected: Vec::new(),
        diagnostics: overlay_diags,
        applied: apply,
        returns: modifiers
            .returns
            .as_ref()
            .map(|r| (String::new(), r.clone())),
        visibility: modifiers
            .visibility
            .as_ref()
            .map(|v| (String::new(), v.clone())),
        asyncness: modifiers.asyncness.map(|a| (!a, a)),
        not_async: Vec::new(),
    })
}
