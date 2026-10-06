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
use std::collections::HashSet;

use super::syntax::{
    is_c_cpp_prototype, is_ident, is_import_or_export_context, is_in_comment, is_in_string,
    keyword_arg, swift_label,
};
use super::types::FoundCall;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn find_calls_in_content(
    content: &str,
    file_rel: &str,
    fn_name: &str,
    target_param_name: &str,
    target_param_index: usize,
    target_label: Option<&str>,
    has_receiver: bool,
    lang: Language,
    is_decl_file: bool,
    decl_open: usize,
    decl_close: usize,
    body_open: usize,
    body_close: usize,
    selected_param_types: &[Option<String>],
    semantic_references: &mut HashSet<(u32, u32)>,
    references_were_empty: bool,
) -> (Vec<FoundCall>, Vec<(usize, usize, String)>, Vec<String>) {
    let mut calls = Vec::new();
    let mut proto_edits = Vec::new();
    let mut unmatched = Vec::new();

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
        if is_in_comment(content, at, lang) || is_in_string(content, at, lang) {
            continue;
        }

        // Line and column for site reporting
        let (lsp_line, lsp_col) = crate::signature::position_at(content, at).unwrap_or((0, 0));
        let line = lsp_line.saturating_sub(1);
        let col = lsp_col;
        let site = format!("{file_rel}:{line}:{col}");

        if is_import_or_export_context(content, at, lang) {
            semantic_references.remove(&(lsp_line, lsp_col));
            continue;
        }

        // Declaration check
        if is_decl_file && at >= decl_open.saturating_sub(fn_name.len() + 20) && at <= decl_close {
            continue;
        }

        // Self-call check inside function's own body
        if is_decl_file && at > body_open && at < body_close {
            if semantic_references.remove(&(lsp_line, lsp_col)) || references_were_empty {
                unmatched.push(format!(
                    "{site} (a call inside `{fn_name}` itself passes its own `{target_param_name}`)"
                ));
            }
            continue;
        }

        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(content, at + fn_name.len())
        else {
            if semantic_references.remove(&(lsp_line, lsp_col)) || references_were_empty {
                unmatched.push(format!(
                    "{site} (the function used as a value: it would change type)"
                ));
            }
            continue;
        };

        if matches!(lang, Language::Cpp | Language::C) && is_c_cpp_prototype(content, at, args_end)
        {
            let (_, proto_params) =
                crate::parameter_object::parse_params(&content[args_start..args_end], lang);
            let proto_types = proto_params
                .iter()
                .map(|p| p.ty.clone())
                .collect::<Vec<_>>();
            if proto_types == selected_param_types
                && let Some(p_idx) = proto_params
                    .iter()
                    .position(|p| p.name == target_param_name)
            {
                let kept: Vec<String> = proto_params
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != p_idx)
                    .map(|(_, p)| p.raw.clone())
                    .collect();
                proto_edits.push((args_start, args_end - args_start, kept.join(", ")));
            }
            continue;
        }

        if !semantic_references.remove(&(lsp_line, lsp_col)) {
            if references_were_empty {
                unmatched.push(format!(
                    "{site} (analyzer references for `{fn_name}` were empty)"
                ));
            }
            continue;
        }

        let args_str = &content[args_start..args_end];
        let args = crate::parameter_object::split_args(args_str);

        let mut found_idx = None;
        let mut found_val = None;

        if lang == Language::Python {
            for (i, a) in args.iter().enumerate() {
                if let Some((k, v)) = keyword_arg(a)
                    && k == target_param_name
                {
                    found_idx = Some(i);
                    found_val = Some(v.to_string());
                    break;
                }
            }
            if found_idx.is_none() {
                let before = content[..at].trim_end();
                let is_method = before.ends_with('.');
                let pos = if has_receiver && !is_method {
                    target_param_index + 1
                } else {
                    target_param_index
                };
                if let Some(a) = args.get(pos) {
                    found_idx = Some(pos);
                    found_val = Some(a.trim().to_string());
                }
            }
        } else if lang == Language::Swift {
            for (i, a) in args.iter().enumerate() {
                if let Some((lbl, v)) = swift_label(a)
                    && (lbl == target_param_name || target_label == Some(lbl))
                {
                    found_idx = Some(i);
                    found_val = Some(v.to_string());
                    break;
                }
            }
            if found_idx.is_none()
                && let Some(a) = args.get(target_param_index)
            {
                found_idx = Some(target_param_index);
                found_val = Some(a.trim().to_string());
            }
        } else if let Some(a) = args.get(target_param_index) {
            found_idx = Some(target_param_index);
            found_val = Some(a.trim().to_string());
        }

        if let (Some(idx), Some(val)) = (found_idx, found_val) {
            calls.push(FoundCall {
                args_start,
                args_end,
                arg_index: idx,
                passed_value: val,
                site,
            });
        } else {
            unmatched.push(format!(
                "{site} (the call has no argument for `{target_param_name}`)"
            ));
        }
    }

    (calls, proto_edits, unmatched)
}
