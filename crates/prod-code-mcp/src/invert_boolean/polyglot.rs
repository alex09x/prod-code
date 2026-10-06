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
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::Language;

use super::decl::find_polyglot_predicate_declaration;
use super::negate::{negate_c_like_body, negate_python_body};
use super::syntax::{
    call_start, is_ident, is_import_export_call_context, is_in_string_or_comment,
    one_based_lsp_position,
};
use super::types::{Inverted, display};

/// Inverts the boolean predicate in polyglot languages: TypeScript/JavaScript, Python, C++, Swift, Go.
#[allow(clippy::too_many_arguments)]
pub async fn invert_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: Option<u32>,
    _character: Option<u32>,
    symbol: Option<&str>,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    anyhow::ensure!(
        !new_name.is_empty() && new_name.chars().all(is_ident),
        "`{new_name}` is not an identifier"
    );
    let lang = Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_predicate_declaration(&text, lang, line, symbol)?;
    let (selected_line, selected_col) = one_based_lsp_position(&text, decl.decl_name_at);
    let mut semantic_references =
        crate::signature::references(remote, root, file, selected_line, selected_col)
            .await?
            .into_iter()
            .map(|(path, ref_line, ref_col)| {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                (path, ref_line, ref_col)
            })
            .collect::<HashSet<_>>();
    let references_are_empty = semantic_references.is_empty();
    let selected_cpp_param_types = if matches!(lang, Language::Cpp | Language::C) {
        let after_name = decl.decl_name_at + decl.fn_name.len();
        let open_paren = after_name
            + text[after_name..decl.close_paren]
                .find('(')
                .unwrap_or_default();
        crate::parameter_object::parse_params(&text[open_paren + 1..decl.close_paren], lang)
            .1
            .into_iter()
            .map(|param| param.ty)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    anyhow::ensure!(decl.fn_name != new_name, "the new name is the old one");

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());

    let (mut negated, mut cancelled) = (0usize, 0usize);
    let mut all_unmatched = Vec::new();

    // Declaration file edits: rename function at declaration and negate body
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((decl.decl_name_at, decl.fn_name.len(), new_name.to_string()));

    let body_slice = &text[decl.body_open + 1..decl.body_close];
    let new_body = if lang == Language::Python {
        negate_python_body(body_slice)
    } else {
        negate_c_like_body(body_slice)
    };
    own.push((
        decl.body_open + 1,
        decl.body_close - decl.body_open - 1,
        new_body,
    ));

    let canonical_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    // Traverse workspace files for calls, prototypes, and imports
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !crate::inline_parameter::language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(&decl.fn_name) {
            continue;
        }

        let is_decl_file =
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == canonical_file;
        let rel_path = display(root, path);

        let mut file_edits = Vec::new();

        for (at, _) in other_content.match_indices(&decl.fn_name) {
            if at > 0 {
                let prev = other_content[..at].chars().next_back().unwrap();
                if is_ident(prev) {
                    continue;
                }
            }
            let after = &other_content[at + decl.fn_name.len()..];
            if after.starts_with(is_ident) {
                continue;
            }
            if is_in_string_or_comment(&other_content, at, lang) {
                continue;
            }

            let source_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let (line, col) = one_based_lsp_position(&other_content, at);
            let reference_key = (source_path, line, col);
            let site = format!("{rel_path}:{line}:{col}");

            if is_import_export_call_context(&other_content, at, lang) {
                if references_are_empty {
                    all_unmatched.push(format!(
                        "{site}: analyzer references for `{}` were empty; nothing was written",
                        decl.fn_name
                    ));
                    continue;
                }
                semantic_references.remove(&reference_key);
                let line_start = other_content[..at].rfind('\n').map_or(0, |n| n + 1);
                let line_end = other_content[at..]
                    .find('\n')
                    .map_or(other_content.len(), |n| at + n);
                if other_content[line_start..line_end].contains(" as ") {
                    all_unmatched.push(format!(
                        "{site} imports `{}` through an alias; alias call sites are not resolved",
                        decl.fn_name
                    ));
                } else {
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                }
                continue;
            }

            // Declaration check in declaring file
            if is_decl_file && at >= decl.decl_name_at && at <= decl.close_paren {
                semantic_references.remove(&reference_key);
                continue;
            }

            if references_are_empty {
                all_unmatched.push(format!(
                    "{site}: analyzer references for `{}` were empty; nothing was written",
                    decl.fn_name
                ));
                continue;
            }

            // Self-call check inside function's own body
            if is_decl_file && at > decl.body_open && at < decl.body_close {
                if semantic_references.remove(&reference_key) {
                    anyhow::bail!(
                        "`{}` calls itself; invert a recursive predicate by hand",
                        decl.fn_name
                    );
                }
                continue;
            }

            let candidate_args =
                crate::parameter_object::call_args_span(&other_content, at + decl.fn_name.len());
            if matches!(lang, Language::Cpp | Language::C)
                && let Some((args_start, args_end)) = candidate_args
                && crate::inline_parameter::is_c_cpp_prototype(&other_content, at, args_end)
            {
                let (_, proto_params) = crate::parameter_object::parse_params(
                    &other_content[args_start..args_end],
                    lang,
                );
                let proto_types = proto_params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect::<Vec<_>>();
                if proto_types == selected_cpp_param_types {
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                }
                continue;
            }

            if !semantic_references.remove(&reference_key) {
                continue;
            }

            let Some((_args_start, args_end)) = candidate_args else {
                all_unmatched.push(format!("{site} `{}` used as a value", decl.fn_name));
                continue;
            };

            // Real call site!
            let begin = call_start(&other_content, at);
            let after_call = other_content[args_end + 1..].trim_start();
            let continues = after_call.starts_with('.')
                || after_call.starts_with('?')
                || after_call.starts_with('[');
            let lead = other_content[..begin].trim_end();

            if lang == Language::Python {
                let is_negated = if let Some(before_not) = lead.strip_suffix("not") {
                    before_not.chars().next_back().is_none_or(|c| !is_ident(c))
                } else {
                    false
                };
                if is_negated {
                    let not_start = lead.len() - 3;
                    file_edits.push((not_start, begin - not_start, String::new()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    cancelled += 1;
                } else if continues {
                    file_edits.push((begin, 0, "(not ".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    file_edits.push((args_end + 1, 0, ")".to_string()));
                    negated += 1;
                } else {
                    file_edits.push((begin, 0, "not ".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    negated += 1;
                }
            } else {
                let is_negated =
                    lead.ends_with('!') && !lead.ends_with("!=") && !lead.ends_with("!==");
                if !continues && is_negated {
                    let not_start = lead.len() - 1;
                    file_edits.push((not_start, begin - not_start, String::new()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    cancelled += 1;
                } else if continues {
                    file_edits.push((begin, 0, "(!".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    file_edits.push((args_end + 1, 0, ")".to_string()));
                    negated += 1;
                } else {
                    file_edits.push((begin, 0, "!".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    negated += 1;
                }
            }
        }

        if !file_edits.is_empty() {
            texts.insert(path.to_path_buf(), other_content);
            edits
                .entry(path.to_path_buf())
                .or_default()
                .extend(file_edits);
        }
    }

    for (path, ref_line, ref_col) in semantic_references {
        all_unmatched.push(format!(
            "{}:{ref_line}:{ref_col}: analyzer reference to `{}` could not be inverted safely",
            display(root, &path),
            decl.fn_name
        ));
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

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
            all_unmatched.is_empty(),
            "{} reference(s) to `{}` were not negated and would mean the opposite; nothing was written:\n  {}",
            all_unmatched.len(),
            decl.fn_name,
            all_unmatched.join("\n  ")
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

    Ok(Inverted {
        was: decl.fn_name,
        now: new_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        kind: "function".to_string(),
        negated,
        cancelled,
        writes: 0,
        blocked: Vec::new(),
        unmatched: all_unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
