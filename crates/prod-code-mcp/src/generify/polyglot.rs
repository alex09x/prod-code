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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::Language;

use super::decl::insert_generic_decl;
use super::param::rewrite_param_entry;
use super::rust::generify_rust;
use super::syntax::{display, find_polyglot_func_decl, is_ident};
use super::types::Generified;

/// Unified generify refactoring across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn generify_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    let lang = crate::parameter_object::Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;

    if lang == Language::Rust {
        let (l, c) = match (line, col) {
            (Some(l), Some(c)) => (l, c),
            (Some(l), None) => (l, 1),
            _ => {
                let text = std::fs::read_to_string(file)
                    .with_context(|| format!("cannot read {}", file.display()))?;
                let sym = symbol.context("missing `symbol` or `line`")?;
                let mut found_pos = None;
                for (name_idx, _) in text.match_indices(sym) {
                    let line_start = text[..name_idx].rfind('\n').map_or(0, |p| p + 1);
                    let before = text[line_start..name_idx].trim();
                    if before.ends_with("fn") || before.ends_with("pub fn") {
                        let (nl, nc) = crate::signature::position_at(&text, name_idx)?;
                        found_pos = Some((nl, nc));
                        break;
                    }
                }
                found_pos.with_context(|| {
                    format!(
                        "could not find declaration of `{sym}` in {}",
                        file.display()
                    )
                })?
            }
        };
        return generify_rust(
            remote, root, file, l, c, param, bound, type_param, apply, force,
        )
        .await;
    }

    anyhow::ensure!(
        !type_param.is_empty() && type_param.chars().all(is_ident),
        "`{type_param}` is not a type parameter name"
    );
    let bound = bound.trim();

    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_func_decl(&text, lang, symbol, line)?;

    // Check collision with existing generics
    if let Some((gs, ge)) = decl.generics_span {
        let existing = &text[gs..ge];
        anyhow::ensure!(
            !existing
                .split(|c: char| !is_ident(c))
                .any(|w| w == type_param),
            "`{}` already has a generic parameter `{type_param}`; pass another `type_param`",
            decl.name
        );
    }

    let list = &text[decl.open_paren + 1..decl.close_paren];
    let (_, params) = crate::parameter_object::parse_params(list, lang);
    let target_idx = params
        .iter()
        .position(|p| p.name == param)
        .with_context(|| format!("`{}` has no parameter `{param}`", decl.name))?;
    let target_param = &params[target_idx];

    let entries = crate::parameter_object::entries(list, lang);
    let entry_match = entries.iter().find(|(at, entry_text)| {
        *at <= target_param.name_at && target_param.name_at <= *at + entry_text.len()
    });
    let (entry_at, entry_text) = entry_match
        .copied()
        .with_context(|| format!("could not locate parameter `{param}` in parameter list"))?;

    let mut new_list = list.to_string();
    let mut parameter_edits = Vec::new();
    if lang == Language::Go {
        let mut group_start = target_idx;
        while group_start > 0 && params[group_start - 1].shares_type {
            group_start -= 1;
        }
        let grouped = group_start < target_idx || target_param.shares_type;
        if grouped {
            let old_type = target_param
                .ty
                .as_deref()
                .context("cannot determine the shared Go parameter type")?;
            for index in group_start..=target_idx {
                let p = &params[index];
                let (at, raw) = entries
                    .iter()
                    .find(|(at, entry)| *at <= p.name_at && p.name_at <= *at + entry.len())
                    .copied()
                    .with_context(|| format!("could not locate Go parameter `{}`", p.name))?;
                let replacement = if index == target_idx {
                    let synthetic = if p.shares_type {
                        format!("{} {old_type}", p.name)
                    } else {
                        raw.to_string()
                    };
                    rewrite_param_entry(&synthetic, p, type_param, lang)
                } else {
                    format!("{} {old_type}", p.name)
                };
                parameter_edits.push((at, raw.len(), replacement));
            }
        } else {
            parameter_edits.push((
                entry_at,
                entry_text.len(),
                rewrite_param_entry(entry_text, target_param, type_param, lang),
            ));
        }
    } else {
        parameter_edits.push((
            entry_at,
            entry_text.len(),
            rewrite_param_entry(entry_text, target_param, type_param, lang),
        ));
    }
    parameter_edits.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    for (at, len, replacement) in parameter_edits {
        new_list.replace_range(at..at + len, &replacement);
    }

    let was = text[decl.decl_start..decl.close_paren + 1]
        .trim()
        .to_string();

    let mut new_text = text.clone();
    new_text.replace_range(decl.open_paren + 1..decl.close_paren, &new_list);

    // Now insert generic parameter declaration
    insert_generic_decl(&mut new_text, &text, &decl, type_param, bound, lang);

    let new_open_p = new_text[decl.decl_start..]
        .find('(')
        .map(|i| decl.decl_start + i)
        .unwrap_or(decl.decl_start);
    let new_close_p =
        crate::parameter_object::matching_bracket(&new_text, new_open_p).unwrap_or(new_open_p);
    let now = new_text[decl.decl_start..new_close_p + 1]
        .trim()
        .to_string();

    let mut rewritten = vec![(file.to_string_lossy().into_owned(), new_text.clone())];

    let mut separate_cpp_header = false;
    if matches!(lang, Language::Cpp | Language::C) {
        let concept_spec = if bound.is_empty() || bound == "typename" || bound == "class" {
            "typename"
        } else {
            bound
        };
        let gen_decl = format!("{concept_spec} {type_param}");

        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let p = entry.path();
            if !p.is_file() || p == file {
                continue;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "h" | "hpp" | "hh" | "hxx") {
                continue;
            }
            let Ok(proto_content) = std::fs::read_to_string(p) else {
                continue;
            };
            if !proto_content.contains(&decl.name) {
                continue;
            }

            if let Ok(proto_decl) =
                find_polyglot_func_decl(&proto_content, lang, Some(&decl.name), None)
            {
                let proto_list = &proto_content[proto_decl.open_paren + 1..proto_decl.close_paren];
                let (_, proto_params) = crate::parameter_object::parse_params(proto_list, lang);
                if let Some(target_proto_param) = proto_params.iter().find(|pr| pr.name == param) {
                    separate_cpp_header = true;
                    let proto_entries = crate::parameter_object::entries(proto_list, lang);
                    if let Some((pr_at, pr_text)) = proto_entries.iter().find(|(at, entry_text)| {
                        *at <= target_proto_param.name_at
                            && target_proto_param.name_at <= *at + entry_text.len()
                    }) {
                        let new_pr_entry =
                            rewrite_param_entry(pr_text, target_proto_param, type_param, lang);
                        let mut new_pr_list = proto_list.to_string();
                        new_pr_list.replace_range(*pr_at..*pr_at + pr_text.len(), &new_pr_entry);

                        let mut new_proto_text = proto_content.clone();
                        new_proto_text.replace_range(
                            proto_decl.open_paren + 1..proto_decl.close_paren,
                            &new_pr_list,
                        );

                        if proto_decl.has_generics {
                            if let Some((_, ge)) = proto_decl.generics_span {
                                new_proto_text.insert_str(ge, &format!(", {gen_decl}"));
                            }
                        } else {
                            let line_start = proto_content[..proto_decl.decl_start]
                                .rfind('\n')
                                .map_or(0, |p| p + 1);
                            let indent_len = proto_content[line_start..].len()
                                - proto_content[line_start..].trim_start().len();
                            let indent = &proto_content[line_start..line_start + indent_len];
                            new_proto_text.insert_str(
                                proto_decl.decl_start,
                                &format!("{indent}template<{gen_decl}>\n"),
                            );
                        }
                        rewritten.push((p.to_string_lossy().into_owned(), new_proto_text));
                    }
                }
            }
        }
    }
    anyhow::ensure!(
        !separate_cpp_header,
        "cannot safely synchronize this C/C++ signature with another header declaration; semantic declaration identity is not available"
    );

    let mut callers_checked = 0usize;
    let mut caller_files = Vec::new();
    let selected_is_header = matches!(
        file.extension().and_then(|ext| ext.to_str()),
        Some("h" | "hpp" | "hh" | "hxx")
    );
    let mut out_of_line_cpp_definition = false;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let p = entry.path();
        if !p.is_file() || p == file || !crate::inline_parameter::language_matches(lang, p) {
            continue;
        }
        if let Ok(other_text) = std::fs::read_to_string(p)
            && other_text.contains(&decl.name)
        {
            callers_checked += 1;
            caller_files.push(p.to_path_buf());
            if selected_is_header
                && matches!(lang, Language::Cpp | Language::C)
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "cpp" | "cc" | "cxx" | "c"))
            {
                out_of_line_cpp_definition = true;
            }
        }
    }
    anyhow::ensure!(
        !out_of_line_cpp_definition,
        "cannot safely generify a C/C++ declaration whose definition is in another source file; move the definition into the header first"
    );

    let files_to_validate: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports =
        crate::diagnostics::validate_texts(remote, root, &files_to_validate, &caller_files).await?;
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
            "the change does not compile ({} error(s)); nothing was written. The body \
             needs more than the bound promises, or a caller no longer satisfies it or can no longer \
             infer its type; choose another bound, fix the caller, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files_map: std::collections::BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&files_map),
        )?;
        applied = true;
    }

    Ok(Generified {
        function: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        callers_checked,
        rewritten,
        diagnostics,
        applied,
    })
}
