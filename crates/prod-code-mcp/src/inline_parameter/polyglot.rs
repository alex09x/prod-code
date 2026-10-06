/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::calls::find_calls_in_content;
use super::decl::{
    brace_insertion_offset_and_indent, find_polyglot_declaration, format_binding,
    python_insertion_offset_and_indent,
};
use super::syntax::{is_caller_independent, language_matches};
use super::types::{InlinedParameter, display};

/// Inlines a parameter in polyglot languages: TypeScript/JavaScript, Python, C++, Swift, Go.
#[allow(clippy::too_many_arguments)]
pub async fn inline_parameter_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: Option<u32>,
    character: Option<u32>,
    function: Option<&str>,
    param: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<InlinedParameter> {
    let lang = Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_declaration(&text, lang, line, function)?;
    let name_at = text[..decl.open_paren]
        .rfind(&decl.fn_name)
        .context("cannot locate the selected function name")?;
    let (reference_line, reference_col) = crate::signature::position_at(&text, name_at)?;
    let mut references_by_file: BTreeMap<PathBuf, HashSet<(u32, u32)>> = BTreeMap::new();
    for (path, ref_line, ref_col) in
        crate::signature::references(remote, root, file, reference_line, reference_col).await?
    {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        references_by_file
            .entry(path)
            .or_default()
            .insert((ref_line, ref_col));
    }
    let references_were_empty = references_by_file
        .values()
        .all(|references| references.is_empty());
    let target_idx = if let Some(p_name) = param {
        decl.params
            .iter()
            .position(|p| p.name == p_name)
            .with_context(|| format!("parameter `{p_name}` not found in `{}`", decl.fn_name))?
    } else if let Some(c) = character
        && let Some(l) = line
    {
        let offset = crate::signature::offset_of(&text, l, c)
            .context("the parameter position is not inside the file")?;
        decl.params
            .iter()
            .position(|p| {
                let start = decl.open_paren + 1 + p.name_at;
                start <= offset && offset <= start + p.name.len()
            })
            .context("the selected position is not inside a parameter name")?
    } else if decl.params.len() == 1 {
        0
    } else {
        anyhow::bail!(
            "multiple parameters in `{}`; specify which parameter to inline using `parameter`",
            decl.fn_name
        );
    };

    let target_param = &decl.params[target_idx];
    let target_param_name = target_param.name.clone();
    let target_param_type = target_param.ty.clone();
    let target_label = target_param.label.clone();
    let has_receiver = decl.receiver.is_some();
    let selected_param_types = decl
        .params
        .iter()
        .map(|param| param.ty.clone())
        .collect::<Vec<_>>();

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut all_unmatched = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();

    let rel_decl_file = display(root, file);
    let canonical_decl = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut decl_references = references_by_file
        .remove(&canonical_decl)
        .unwrap_or_default();
    let (decl_calls, decl_protos, decl_unmatched) = find_calls_in_content(
        &text,
        &rel_decl_file,
        &decl.fn_name,
        &target_param_name,
        target_idx,
        target_label.as_deref(),
        has_receiver,
        lang,
        true,
        decl.open_paren,
        decl.close_paren,
        decl.body_open,
        decl.body_close,
        &selected_param_types,
        &mut decl_references,
        references_were_empty,
    );
    all_unmatched.extend(decl_unmatched);
    for (ref_line, ref_col) in decl_references {
        all_unmatched.push(format!(
            "{rel_decl_file}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely"
        ));
    }
    for (p_at, p_len, p_rep) in decl_protos {
        edits
            .entry(file.to_path_buf())
            .or_default()
            .push((p_at, p_len, p_rep));
    }
    for call in decl_calls {
        values.push((call.site, call.passed_value));
        let args_str = &text[call.args_start..call.args_end];
        let args = crate::parameter_object::split_args(args_str);
        let remaining: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != call.arg_index)
            .map(|(_, a)| a.trim())
            .collect();
        edits.entry(file.to_path_buf()).or_default().push((
            call.args_start,
            call.args_end - call.args_start,
            remaining.join(", "),
        ));
    }

    // Search workspace files for calls
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || path == file || !language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(&decl.fn_name) {
            continue;
        }
        let rel_other = display(root, path);
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut other_references = references_by_file.remove(&canonical).unwrap_or_default();
        let (other_calls, other_protos, other_unmatched) = find_calls_in_content(
            &other_content,
            &rel_other,
            &decl.fn_name,
            &target_param_name,
            target_idx,
            target_label.as_deref(),
            has_receiver,
            lang,
            false,
            0,
            0,
            0,
            0,
            &selected_param_types,
            &mut other_references,
            references_were_empty,
        );
        all_unmatched.extend(other_unmatched);
        for (ref_line, ref_col) in other_references {
            all_unmatched.push(format!(
                "{rel_other}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely"
            ));
        }
        if !other_calls.is_empty() || !other_protos.is_empty() {
            texts.insert(path.to_path_buf(), other_content.clone());
            for (p_at, p_len, p_rep) in other_protos {
                edits
                    .entry(path.to_path_buf())
                    .or_default()
                    .push((p_at, p_len, p_rep));
            }
            for call in other_calls {
                values.push((call.site, call.passed_value));
                let args_str = &other_content[call.args_start..call.args_end];
                let args = crate::parameter_object::split_args(args_str);
                let remaining: Vec<&str> = args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != call.arg_index)
                    .map(|(_, arg)| arg.trim())
                    .collect();
                edits.entry(path.to_path_buf()).or_default().push((
                    call.args_start,
                    call.args_end - call.args_start,
                    remaining.join(", "),
                ));
            }
        }
    }

    for (path, references) in references_by_file {
        for (ref_line, ref_col) in references {
            all_unmatched.push(format!(
                "{}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely",
                display(root, &path)
            ));
        }
    }

    let first = values.first().map(|(_, v)| v.clone()).with_context(|| {
        format!("no call passes a value for `{target_param_name}`, so there is none to inline")
    })?;
    let differing: Vec<String> = values
        .iter()
        .filter(|(_, v)| *v != first)
        .map(|(site, v)| format!("{site} passes `{v}`"))
        .collect();
    anyhow::ensure!(
        differing.is_empty(),
        "the calls do not agree on `{target_param_name}`: {} of {} pass `{first}`, and\n  {}",
        values.len() - differing.len(),
        values.len(),
        differing.join("\n  ")
    );
    anyhow::ensure!(
        is_caller_independent(&first),
        "every call passes `{first}` for `{target_param_name}`, but it may name something of the caller's (a local, or an expression over one); only a literal, a constant or a path is inlined"
    );

    // Declaration edits in declaring file
    let mut kept_params = Vec::new();
    if let Some(r) = &decl.receiver {
        kept_params.push(r.clone());
    }
    for (i, p) in decl.params.iter().enumerate() {
        if i != target_idx {
            if lang == Language::Go && p.shares_type {
                if let Some(ty) = &p.ty {
                    kept_params.push(format!("{} {}", p.name, ty));
                } else {
                    kept_params.push(p.raw.clone());
                }
            } else {
                kept_params.push(p.raw.clone());
            }
        }
    }

    let (insert_offset, indent) = if lang == Language::Python {
        python_insertion_offset_and_indent(&text, decl.body_open, decl.open_paren)
    } else {
        brace_insertion_offset_and_indent(&text, decl.body_open, decl.body_close, lang)
    };

    let binding = format_binding(
        &target_param_name,
        target_param_type.as_deref(),
        &first,
        lang,
    );
    let insertion_text = if lang == Language::Python {
        format!("{indent}{binding}\n")
    } else {
        format!("\n{indent}{binding}")
    };

    let own_edits = edits.entry(file.to_path_buf()).or_default();
    own_edits.push((
        decl.open_paren + 1,
        decl.close_paren - (decl.open_paren + 1),
        kept_params.join(", "),
    ));
    own_edits.push((insert_offset, 0, insertion_text));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }
    let rewritten_calls = values.len();

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
            "{} reference(s) to `{}` are not a call passing `{target_param_name}`; nothing was written:\n  {}",
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

    Ok(InlinedParameter {
        function: decl.fn_name,
        parameter: target_param_name,
        value: first,
        root: root.to_path_buf(),
        file: rel_decl_file,
        rewritten_calls,
        unmatched: all_unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
