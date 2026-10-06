/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::parameter_object::{Language, call_args_span, split_args};
use crate::signature::{Modifiers, Param};

use super::sources::is_c_cpp_prototype;
use super::syntax::{
    display, is_ident, is_import_or_export_context, is_in_comment, is_side_effect_free_argument,
    one_based_lsp_position,
};
use super::types::PolyglotDecl;

#[allow(clippy::too_many_arguments)]
pub fn rewrite_file_calls(
    root: &Path,
    src_path: &Path,
    content: &str,
    is_decl_file: bool,
    decl: &PolyglotDecl,
    old_signature: &str,
    new_signature: &str,
    decl_edits: &[(usize, usize, String)],
    request: &[Param],
    modifiers: &Modifiers,
    lang: Language,
    semantic_references: &mut HashSet<(PathBuf, u32, u32)>,
    unmatched: &mut Vec<String>,
) -> Option<String> {
    let mut file_edits: Vec<(usize, usize, String)> = if is_decl_file {
        decl_edits.to_vec()
    } else {
        Vec::new()
    };

    let fn_name = &decl.fn_name;
    for (at, _) in content.match_indices(fn_name) {
        if at > 0 {
            let prev = content[..at].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after = &content[at + fn_name.len()..];
        if after.starts_with(is_ident) {
            continue;
        }

        let source_path =
            std::fs::canonicalize(src_path).unwrap_or_else(|_| src_path.to_path_buf());
        let (line_num, col_num) = one_based_lsp_position(content, at);
        let reference_key = (source_path, line_num, col_num);
        if is_import_or_export_context(content, at, lang) {
            semantic_references.remove(&reference_key);
            continue;
        }
        // Skip comments and strings.
        if is_in_comment(content, at, lang)
            || crate::inline_parameter::is_in_string(content, at, lang)
        {
            continue;
        }

        // Skip declaration itself in declaration file
        if is_decl_file
            && at >= decl.open_paren.saturating_sub(fn_name.len() + 20)
            && at <= decl.close_paren
        {
            continue;
        }

        // Check if call args follow
        let Some((args_start, args_end)) = call_args_span(content, at + fn_name.len()) else {
            continue;
        };
        let site = format!("{}:{line_num}:{col_num}", display(root, src_path));

        // Check if C/C++ prototype
        if matches!(lang, Language::Cpp | Language::C)
            && is_c_cpp_prototype(content, at, args_start, args_end)
        {
            if content[args_start..args_end].trim() != old_signature.trim() {
                continue;
            }
            let after_paren = content[args_end + 1..].trim_start();
            if after_paren.starts_with(';') {
                // It's a prototype: rewrite prototype parameter list and return type
                file_edits.push((args_start, args_end, new_signature.to_string()));
                if let Some(new_ret) = &modifiers.returns {
                    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
                    let before_fn = &content[line_start..at];
                    let indent = before_fn
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect::<String>();
                    file_edits.push((line_start, at, format!("{indent}{new_ret} ")));
                }
                continue;
            }
        }

        if !semantic_references.remove(&reference_key) {
            continue;
        }

        // Call site rewriting
        let old_args_str = &content[args_start..args_end];
        let old_args = split_args(old_args_str);
        let is_python_keyword_call =
            lang == Language::Python && old_args.iter().any(|a| a.split_once('=').is_some());
        let kept_indices = request
            .iter()
            .filter_map(|r| match r {
                Param::Keep(name) => decl.params.iter().position(|d| &d.name == name),
                Param::Add { .. } => None,
            })
            .collect::<Vec<_>>();
        let reordered = kept_indices.windows(2).any(|pair| pair[0] > pair[1]);
        let dropped = kept_indices.len() < decl.params.len();
        let effectful_existing = old_args
            .iter()
            .any(|arg| !is_side_effect_free_argument(arg, lang));
        let effectful_added = request.iter().any(
            |r| matches!(r, Param::Add { value, .. } if !is_side_effect_free_argument(value, lang)),
        );
        if ((reordered || dropped) && effectful_existing) || effectful_added {
            unmatched.push(format!(
                "{site} has argument side effects that cannot safely survive this signature change"
            ));
            continue;
        }

        let mut new_args = Vec::new();
        for r in request {
            match r {
                Param::Keep(name) => {
                    let orig_idx = decl.params.iter().position(|d| &d.name == name);
                    let orig_param = decl.params.iter().find(|d| &d.name == name);

                    // Look up argument in old_args
                    let mut arg_val = None;
                    if lang == Language::Python {
                        // First check keyword arguments
                        for a in &old_args {
                            let a_trimmed = a.trim();
                            if let Some((k, _v)) = a_trimmed.split_once('=')
                                && k.trim() == name
                            {
                                arg_val = Some(a_trimmed.to_string());
                                break;
                            }
                        }
                        if arg_val.is_none()
                            && let Some(idx) = orig_idx
                        {
                            let pos = if decl.receiver.is_some()
                                && !content[..at].trim_end().ends_with('.')
                            {
                                idx + 1
                            } else {
                                idx
                            };
                            if let Some(a) = old_args.get(pos) {
                                let a_trimmed = a.trim();
                                if is_python_keyword_call {
                                    arg_val = Some(format!("{name}={a_trimmed}"));
                                } else {
                                    arg_val = Some(a_trimmed.to_string());
                                }
                            }
                        }
                    } else if lang == Language::Swift {
                        if let Some(orig_p) = orig_param {
                            for a in &old_args {
                                let a_trimmed = a.trim();
                                if let Some((lbl, val)) = a_trimmed.split_once(':')
                                    && (lbl.trim() == name
                                        || orig_p.label.as_deref() == Some(lbl.trim()))
                                {
                                    arg_val = Some(format!("{}: {}", lbl.trim(), val.trim()));
                                    break;
                                }
                            }
                            if arg_val.is_none()
                                && let Some(idx) = orig_idx
                                && let Some(a) = old_args.get(idx)
                            {
                                let a_trimmed = a.trim();
                                if let Some(lbl) = &orig_p.label {
                                    if lbl != "_" {
                                        arg_val = Some(format!("{lbl}: {a_trimmed}"));
                                    } else {
                                        arg_val = Some(a_trimmed.to_string());
                                    }
                                } else {
                                    arg_val = Some(format!("{name}: {a_trimmed}"));
                                }
                            }
                        }
                    } else if let Some(idx) = orig_idx
                        && let Some(a) = old_args.get(idx)
                    {
                        arg_val = Some(a.trim().to_string());
                    }

                    if let Some(val) = arg_val {
                        new_args.push(val);
                    } else if let Some(orig_p) = orig_param
                        && let Some(def) = &orig_p.default
                    {
                        new_args.push(def.clone());
                    }
                }
                Param::Add { name, ty: _, value } => {
                    if lang == Language::Swift {
                        new_args.push(format!("{name}: {value}"));
                    } else if is_python_keyword_call {
                        new_args.push(format!("{name}={value}"));
                    } else {
                        new_args.push(value.clone());
                    }
                }
            }
        }

        let new_call_args = new_args.join(", ");
        file_edits.push((args_start, args_end, new_call_args));

        // Check if call needs `await`
        if modifiers.asyncness == Some(true) {
            let before_call = content[..at].trim_end();
            if !before_call.ends_with("await") {
                let after_call = content[args_end + 1..].trim_start();
                if after_call.starts_with('.') || after_call.starts_with('[') {
                    // Parenthesize
                    file_edits.push((at, at, "(await ".to_string()));
                    file_edits.push((args_end + 1, args_end + 1, ")".to_string()));
                } else {
                    file_edits.push((at, at, "await ".to_string()));
                }
            }
        } else if modifiers.asyncness == Some(false) {
            let before_call = content[..at].trim_end();
            if let Some(await_start) = before_call.strip_suffix("await").map(str::len)
                && !content[..await_start]
                    .chars()
                    .next_back()
                    .is_some_and(is_ident)
            {
                file_edits.push((await_start, at, String::new()));
            }
        }
    }

    if file_edits.is_empty() {
        return None;
    }

    // Sort edits in descending order of start offset to avoid shifting
    file_edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut new_content = content.to_string();
    for (start, end, replacement) in file_edits {
        if start <= end && end <= new_content.len() {
            new_content.replace_range(start..end, &replacement);
        }
    }
    Some(new_content)
}
