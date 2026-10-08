/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::conversion::type_name;
use super::{contains_ident, find_enclosing_fn, find_matching_vars};
use crate::parameter_object::Language;

pub(crate) fn find_matching_return(
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
) -> Option<(usize, usize, String)> {
    let old_ty_norm = type_name(old_ty);

    for (idx, _) in text.match_indices(sym) {
        if !contains_ident(&text[idx..idx + sym.len()], sym) {
            continue;
        }
        let line_start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
        let before = text[line_start..idx].trim();
        let line_end = text[idx..].find('\n').map_or(text.len(), |p| idx + p);
        let is_return = before.starts_with("return")
            || (lang == Language::Rust && {
                let after = text[idx + sym.len()..line_end].trim();
                after.is_empty() || after == "}"
            });
        if !is_return {
            continue;
        }

        if let Some(fn_decl) = find_enclosing_fn(text, idx, lang)
            && type_name(&fn_decl.return_type) == old_ty_norm
        {
            return Some((fn_decl.ret_start, fn_decl.ret_end, fn_decl.name));
        }
    }
    None
}

pub(crate) fn find_matching_caller_vars(
    text: &str,
    fn_name: &str,
    old_ty: &str,
    lang: Language,
) -> Vec<(usize, usize, String)> {
    let mut results = Vec::new();

    for (idx, _) in text.match_indices(fn_name) {
        if !contains_ident(&text[idx..idx + fn_name.len()], fn_name) {
            continue;
        }
        let after = text[idx + fn_name.len()..].trim_start();
        if !after.starts_with('(') {
            continue;
        }
        let line_start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
        let line_end = text[idx..].find('\n').map_or(text.len(), |p| idx + p);
        let line = &text[line_start..line_end];
        let vars = find_matching_vars(line, fn_name, old_ty, lang);
        for (s_rel, e_rel, var_name) in vars {
            results.push((line_start + s_rel, line_start + e_rel, var_name));
        }
    }
    results
}
