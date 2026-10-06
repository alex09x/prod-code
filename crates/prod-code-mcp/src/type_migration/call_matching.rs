/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::conversion::type_name;
use super::matching::contains_ident;
use super::spans::{declared_type_span_polyglot, matching};
use super::types::ParamInfo;
use crate::parameter_object::Language;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) fn extract_param_info(
    text: &str,
    fn_name: &str,
    arg_index: usize,
    lang: Language,
) -> Option<ParamInfo> {
    for (fn_idx, _) in text.match_indices(fn_name) {
        if !contains_ident(&text[fn_idx..fn_idx + fn_name.len()], fn_name) {
            continue;
        }
        let after_name = &text[fn_idx + fn_name.len()..];
        let trimmed = after_name.trim_start();
        if !trimmed.starts_with('(') {
            continue;
        }
        let open_p = fn_idx + fn_name.len() + (after_name.len() - trimmed.len());
        let close_p = matching(text, open_p)?;
        let params_text = &text[open_p + 1..close_p];
        let params: Vec<&str> = params_text.split(',').collect();
        if arg_index >= params.len() {
            return None;
        }
        let target_param = params[arg_index].trim();
        let param_offset = open_p + 1 + text[open_p + 1..close_p].find(target_param)?;
        let (s, e) = declared_type_span_polyglot(text, param_offset, lang)?;
        let name: String = target_param
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return Some(ParamInfo {
            name,
            ty: text[s..e].trim().to_string(),
            start: s,
            end: e,
        });
    }
    None
}

pub(crate) fn find_matching_call_params(
    root: &Path,
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
    rewritten: &BTreeMap<PathBuf, String>,
) -> Vec<(PathBuf, usize, usize, String, String)> {
    let mut results = Vec::new();
    let old_ty_norm = type_name(old_ty);

    for (idx, _) in text.match_indices(sym) {
        if !contains_ident(&text[idx..idx + sym.len()], sym) {
            continue;
        }
        let prefix = &text[..idx];
        let Some(open_p) = prefix.rfind('(') else {
            continue;
        };
        let before_p = text[..open_p].trim_end();
        let callee_name: String = before_p
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if callee_name.is_empty() {
            continue;
        }

        let args_slice = &text[open_p + 1..idx];
        let arg_index = args_slice.split(',').count() - 1;

        let candidate_files = collect_candidate_files(root, rewritten, &callee_name);
        for cf in candidate_files {
            let cf_text = match rewritten.get(&cf) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&cf) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            let cf_lang = Language::of(&cf).unwrap_or(lang);
            if let Some(param_info) = extract_param_info(&cf_text, &callee_name, arg_index, cf_lang)
                && type_name(&param_info.ty) == old_ty_norm
            {
                results.push((
                    cf.clone(),
                    param_info.start,
                    param_info.end,
                    param_info.name,
                    callee_name.clone(),
                ));
            }
        }
    }
    results
}

pub(crate) fn collect_candidate_files(
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    name: &str,
) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for p in rewritten.keys() {
        files.insert(p.clone());
    }
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let p = entry.path();
        if p.is_file() {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(
                ext,
                "rs" | "ts"
                    | "js"
                    | "tsx"
                    | "jsx"
                    | "py"
                    | "go"
                    | "cpp"
                    | "c"
                    | "h"
                    | "hpp"
                    | "swift"
            ) && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(name)
            {
                files.insert(p.to_path_buf());
            }
        }
    }
    files.into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_cpp_headers(
    root: &Path,
    source_file: &Path,
    fn_name: &str,
    was: &str,
    to: &str,
    rewritten: &mut BTreeMap<PathBuf, String>,
    also: &mut Vec<PathBuf>,
    _is_return: bool,
) {
    let header_candidates = [
        source_file.with_extension("h"),
        source_file.with_extension("hpp"),
    ];
    for h in &header_candidates {
        let h_path = if h.exists() {
            Some(h.clone())
        } else {
            let stem = source_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let inc_h = root.join("include").join(format!("{stem}.h"));
            if inc_h.exists() { Some(inc_h) } else { None }
        };
        if let Some(h_file) = h_path {
            let h_text = match rewritten.get(&h_file) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&h_file) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            for (idx, _) in h_text.match_indices(fn_name) {
                if let Some((start, end)) = declared_type_span_polyglot(&h_text, idx, Language::Cpp)
                    && h_text[start..end].trim() == was.trim()
                {
                    let mut updated_h = h_text.clone();
                    updated_h.replace_range(start..end, to);
                    rewritten.insert(h_file.clone(), updated_h);
                    if !also.contains(&h_file) {
                        also.push(h_file);
                    }
                    break;
                }
            }
        }
    }
}
