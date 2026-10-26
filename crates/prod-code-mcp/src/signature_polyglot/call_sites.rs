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

use crate::parameter_object::{Language, split_args};
use crate::signature::{Modifiers, Param};

use super::sources::is_c_cpp_prototype;
use super::syntax::{
    call_args_span, display, is_ident, is_import_or_export_context_with_jsx, is_in_comment,
    is_side_effect_free_argument, one_based_lsp_position, python_keyword,
};
use super::types::PolyglotDecl;

#[allow(clippy::too_many_arguments)]
pub fn rewrite_file_calls(
    root: &Path,
    src_path: &Path,
    content: &str,
    is_decl_file: bool,
    decl: &PolyglotDecl,
    decl_edits: &[(usize, usize, String)],
    request: &[Param],
    modifiers: &Modifiers,
    lang: Language,
    force: bool,
    semantic_references: &mut HashSet<(PathBuf, u32, u32)>,
    unmatched: &mut Vec<String>,
) -> Option<String> {
    // A caller is read as its own file's language, not the declaration's: a `.js` caller of a
    // TypeScript function holds JSX, and a `.ts` caller of a JavaScript one `<T>x` assertions
    // (#1407).
    let lang = match (lang, Language::of(src_path)) {
        (
            Language::JavaScript | Language::TypeScript,
            Some(own @ (Language::JavaScript | Language::TypeScript)),
        ) => own,
        _ => lang,
    };
    let jsx = lang == Language::JavaScript
        || matches!(
            src_path.extension().and_then(|s| s.to_str()),
            Some("jsx" | "tsx")
        );
    let mut file_edits: Vec<(usize, usize, String)> = if is_decl_file {
        decl_edits.to_vec()
    } else {
        Vec::new()
    };

    let fn_name = &decl.fn_name;
    let mut matched_calls = Vec::new();
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
        if is_import_or_export_context_with_jsx(content, at, lang, jsx) {
            semantic_references.remove(&reference_key);
            continue;
        }
        // Semantic JS/TS references can be inside JSX or template substitutions.
        // The expression lexer below understands those executable regions;
        // whole-line string/comment heuristics must not hide them.
        if !matches!(lang, Language::JavaScript | Language::TypeScript)
            && (is_in_comment(content, at, lang)
                || crate::inline_parameter::is_in_string(content, at, lang))
        {
            continue;
        }

        // Only the selected declaration token denotes the declaration itself.
        if is_decl_file && at == decl.name_start {
            semantic_references.remove(&reference_key);
            continue;
        }
        // A call in a replaced parameter/default span cannot be applied as an
        // independent edit. Retain its reference for the atomic unmatched guard.
        if is_decl_file
            && decl_edits
                .iter()
                .any(|(start, end, _)| *start <= at && at < *end)
        {
            continue;
        }

        // Check if call args follow
        let Some((args_start, args_end, _)) =
            call_arguments_at(content, at + fn_name.len(), lang, jsx)
        else {
            continue;
        };
        let site = format!("{}:{line_num}:{col_num}", display(root, src_path));

        // Every declaration and call must first belong to the selected symbol.
        if !semantic_references.remove(&reference_key) {
            continue;
        }

        if matches!(lang, Language::Cpp | Language::C)
            && (is_c_cpp_prototype(content, at, args_start, args_end)
                || content[args_end + 1..].trim_start().starts_with('{'))
        {
            match super::cpp_declarations::edits(
                content, line_num, col_num, args_start, decl, request, modifiers, lang, force,
            ) {
                Ok(edits) => file_edits.extend(edits),
                Err(error) => unmatched.push(format!("{site}: {error}")),
            }
            continue;
        }

        matched_calls.push((at, line_num, col_num, args_start, args_end));
    }

    for (at, line_num, col_num, args_start, args_end) in matched_calls.into_iter().rev() {
        let site = format!("{}:{line_num}:{col_num}", display(root, src_path));
        let Some((_, _, mut old_args)) = call_arguments_at(content, at + fn_name.len(), lang, jsx)
        else {
            unmatched.push(format!("{site} call arguments could not be parsed safely"));
            continue;
        };

        // Call site rewriting
        let is_python_keyword_call =
            lang == Language::Python && old_args.iter().any(|a| python_keyword(a).is_some());
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

        let unchanged_params = request.len() == decl.params.len()
            && request
                .iter()
                .zip(&decl.params)
                .all(|(r, p)| matches!(r, Param::Keep(name) if *name == p.name));
        if !unchanged_params {
            if file_edits.iter().any(|(s, e, _)| {
                !(*e <= args_start || *s >= args_end) && !(args_start <= *s && *e <= args_end)
            }) {
                unmatched.push(format!("{site} has overlapping call edits"));
                continue;
            }
            let mut inner_edits = Vec::new();
            file_edits.retain(|edit| {
                if args_start <= edit.0 && edit.1 <= args_end {
                    inner_edits.push(edit.clone());
                    false
                } else {
                    true
                }
            });
            if !inner_edits.is_empty() {
                inner_edits.sort_by_key(|(s, e, _)| std::cmp::Reverse((*s, *e)));
                let mut inner_content = content.to_string();
                for (s, e, replacement) in inner_edits {
                    inner_content.replace_range(s..e, &replacement);
                }
                let Some((_, _, composed_args)) =
                    call_arguments_at(&inner_content, at + fn_name.len(), lang, jsx)
                else {
                    unmatched.push(format!("{site} has overlapping call edits"));
                    continue;
                };
                old_args = composed_args;
            }
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
                            if let Some((k, _v)) = python_keyword(a_trimmed)
                                && k == name
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
                            if let Some(a) = old_args
                                .iter()
                                .filter(|a| python_keyword(a).is_none())
                                .nth(pos)
                            {
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
        if !unchanged_params {
            file_edits.push((args_start, args_end, new_call_args));
        }

        // Check if call needs `await`
        if let Some(want_async) = modifiers.asyncness {
            match super::await_operand::edits(
                content,
                at,
                args_start,
                args_end + 1,
                lang,
                want_async,
                jsx,
            ) {
                Some(edits) => file_edits.extend(edits),
                None => unmatched.push(format!("{site} has an unresolved await operand")),
            }
        }
    }

    if file_edits.is_empty() {
        return None;
    }

    for (i, (s1, e1, _)) in file_edits.iter().enumerate() {
        if file_edits[i + 1..]
            .iter()
            .any(|(s2, e2, _)| edits_overlap(*s1, *e1, *s2, *e2))
        {
            unmatched.push(format!(
                "{}: overlapping edits could not be applied safely",
                display(root, src_path)
            ));
            return None;
        }
    }

    // Sort edits in descending order of (start, end) to avoid shifting
    file_edits.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
    let mut new_content = content.to_string();
    for (start, end, replacement) in file_edits {
        if start <= end && end <= new_content.len() {
            new_content.replace_range(start..end, &replacement);
        }
    }
    Some(new_content)
}

fn call_arguments_at(
    content: &str,
    after_name: usize,
    lang: Language,
    jsx: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if matches!(lang, Language::JavaScript | Language::TypeScript) {
        super::js_syntax::call_arguments(content, after_name, lang, jsx)
    } else {
        call_args_span(content, after_name, lang)
            .map(|(start, end)| (start, end, split_args(&content[start..end])))
    }
}

fn edits_overlap(s1: usize, e1: usize, s2: usize, e2: usize) -> bool {
    (s1 < e1 && s2 < e2 && s1.max(s2) < e1.min(e2))
        || (s1 < e1 && ((s1 < s2 && s2 < e1) || (s1 < e2 && e2 < e1)))
        || (s2 < e2 && ((s2 < s1 && s1 < e2) || (s2 < e1 && e1 < e2)))
}
