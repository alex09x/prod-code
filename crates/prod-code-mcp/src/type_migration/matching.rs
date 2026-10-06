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
use super::spans::matching;
use super::types::EnclosingFn;
use crate::parameter_object::Language;

pub(crate) fn contains_ident(haystack: &str, ident: &str) -> bool {
    for (idx, _) in haystack.match_indices(ident) {
        let before_ok =
            idx == 0 || !haystack[..idx].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let after_idx = idx + ident.len();
        let after_ok = after_idx == haystack.len()
            || !haystack[after_idx..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

pub(crate) fn is_valid_ident(s: &str) -> bool {
    !s.is_empty()
        && (s.chars().next().unwrap().is_alphabetic() || s.starts_with('_'))
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

pub(crate) fn find_matching_vars(
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
) -> Vec<(usize, usize, String)> {
    let mut results = Vec::new();
    let old_ty_norm = type_name(old_ty);

    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }
        if !contains_ident(line, sym) {
            continue;
        }
        let line_offset = text
            .lines()
            .take(line_no)
            .map(|l| l.len() + 1)
            .sum::<usize>();

        match lang {
            Language::Rust => {
                if let Some(rest) = trimmed.strip_prefix("let ") {
                    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
                    if let Some(colon) = rest.find(':') {
                        let var_name = rest[..colon].trim().to_string();
                        if let Some(eq) = rest.find('=')
                            && colon < eq
                        {
                            let raw_ty = &rest[colon + 1..eq];
                            let ty_str = raw_ty.trim();
                            if type_name(ty_str) == old_ty_norm
                                && contains_ident(&rest[eq + 1..], sym)
                                && let Some(rel) = line.find(raw_ty)
                            {
                                let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                                let s = line_offset + s_rel;
                                let e = s + ty_str.len();
                                results.push((s, e, var_name));
                            }
                        }
                    }
                }
            }
            Language::TypeScript | Language::JavaScript => {
                let rest_opt = trimmed
                    .strip_prefix("const ")
                    .or_else(|| trimmed.strip_prefix("let "))
                    .or_else(|| trimmed.strip_prefix("var "));
                if let Some(rest) = rest_opt
                    && let Some(colon) = rest.find(':')
                {
                    let var_name = rest[..colon].trim().to_string();
                    if let Some(eq) = rest.find('=')
                        && colon < eq
                    {
                        let raw_ty = &rest[colon + 1..eq];
                        let ty_str = raw_ty.trim();
                        if type_name(ty_str) == old_ty_norm
                            && contains_ident(&rest[eq + 1..], sym)
                            && let Some(rel) = line.find(raw_ty)
                        {
                            let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                            let s = line_offset + s_rel;
                            let e = s + ty_str.len();
                            results.push((s, e, var_name));
                        }
                    }
                }
            }
            Language::Python => {
                if let Some(colon) = trimmed.find(':') {
                    let var_name = trimmed[..colon].trim().to_string();
                    if !var_name.contains(' ')
                        && is_valid_ident(&var_name)
                        && let Some(eq) = trimmed.find('=')
                        && colon < eq
                    {
                        let raw_ty = &trimmed[colon + 1..eq];
                        let ty_str = raw_ty.trim();
                        if type_name(ty_str) == old_ty_norm
                            && contains_ident(&trimmed[eq + 1..], sym)
                            && let Some(rel) = line.find(raw_ty)
                        {
                            let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                            let s = line_offset + s_rel;
                            let e = s + ty_str.len();
                            results.push((s, e, var_name));
                        }
                    }
                }
            }
            Language::Swift => {
                let rest_opt = trimmed
                    .strip_prefix("let ")
                    .or_else(|| trimmed.strip_prefix("var "));
                if let Some(rest) = rest_opt
                    && let Some(colon) = rest.find(':')
                {
                    let var_name = rest[..colon].trim().to_string();
                    if let Some(eq) = rest.find('=')
                        && colon < eq
                    {
                        let raw_ty = &rest[colon + 1..eq];
                        let ty_str = raw_ty.trim();
                        if type_name(ty_str) == old_ty_norm
                            && contains_ident(&rest[eq + 1..], sym)
                            && let Some(rel) = line.find(raw_ty)
                        {
                            let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                            let s = line_offset + s_rel;
                            let e = s + ty_str.len();
                            results.push((s, e, var_name));
                        }
                    }
                }
            }
            Language::Go => {
                if let Some(rest) = trimmed.strip_prefix("var ")
                    && let Some(eq) = rest.find('=')
                {
                    let before_eq = rest[..eq].trim();
                    let parts: Vec<&str> = before_eq.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let var_name = parts[0].to_string();
                        let ty_str = parts[1..].join(" ");
                        if type_name(&ty_str) == old_ty_norm
                            && contains_ident(&rest[eq + 1..], sym)
                            && let Some(var_pos) = line.find(&var_name)
                        {
                            let after_var = var_pos + var_name.len();
                            if let Some(rel) = line[after_var..].find(&ty_str) {
                                let s_rel = after_var + rel;
                                let s = line_offset + s_rel;
                                let e = s + ty_str.len();
                                results.push((s, e, var_name));
                            }
                        }
                    }
                }
            }
            Language::Cpp | Language::C | Language::Java => {
                if let Some(eq) = trimmed.find('=') {
                    let before_eq = trimmed[..eq].trim();
                    let parts: Vec<&str> = before_eq.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let var_name = parts.last().unwrap().to_string();
                        let ty_str = parts[..parts.len() - 1].join(" ");
                        if type_name(&ty_str) == old_ty_norm
                            && contains_ident(&trimmed[eq + 1..], sym)
                            && let Some(s_rel) = line.find(&ty_str)
                        {
                            let s = line_offset + s_rel;
                            let e = s + ty_str.len();
                            results.push((s, e, var_name));
                        }
                    }
                }
            }
        }
    }

    results
}

pub(crate) fn find_enclosing_fn(text: &str, at: usize, lang: Language) -> Option<EnclosingFn> {
    let prefix = &text[..at];
    match lang {
        Language::Rust => {
            let fn_idx = prefix.rfind("fn ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 3 + header[3..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 3..open_p].trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let arrow = text[close_p..body_open].find("->")? + close_p;
            let start = arrow + 2;
            let start =
                start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open
                - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn {
                name,
                return_type,
                ret_start: start,
                ret_end: end,
            })
        }
        Language::Swift => {
            let fn_idx = prefix.rfind("func ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 5 + header[5..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 5..open_p].trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let arrow = text[close_p..body_open].find("->")? + close_p;
            let start = arrow + 2;
            let start =
                start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open
                - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn {
                name,
                return_type,
                ret_start: start,
                ret_end: end,
            })
        }
        Language::Python => {
            let fn_idx = prefix.rfind("def ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 4 + header[4..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 4..open_p].trim().to_string();
            let colon = text[close_p..at].find(':')? + close_p;
            let between = &text[close_p..colon];
            let arrow = between.find("->")? + close_p;
            let start = arrow + 2;
            let start = start + text[start..].len() - text[start..].trim_start().len();
            let end = colon - (text[start..colon].len() - text[start..colon].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn {
                name,
                return_type,
                ret_start: start,
                ret_end: end,
            })
        }
        Language::TypeScript | Language::JavaScript => {
            let fn_idx = prefix
                .rfind("function ")
                .or_else(|| prefix.rfind("const "))
                .or_else(|| prefix.rfind("let "))?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + header.find('(')?;
            let close_p = matching(text, open_p)?;
            let name_part = if prefix[fn_idx..].starts_with("function ") {
                &text[fn_idx + 9..open_p]
            } else {
                let eq = header.find('=')?;
                header[..eq].split_whitespace().last()?
            };
            let name = name_part.trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let colon = text[close_p..body_open].find(':')? + close_p;
            let start = colon + 1;
            let start =
                start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open
                - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn {
                name,
                return_type,
                ret_start: start,
                ret_end: end,
            })
        }
        Language::Go => {
            let fn_idx = prefix.rfind("func ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 5 + header[5..].find('(')?;
            let close_p = matching(text, open_p)?;
            let body_open = text[close_p..at].find('{')? + close_p;
            let between = &text[close_p + 1..body_open];
            let name = text[fn_idx + 5..open_p].trim().to_string();
            let start = close_p + 1 + (between.len() - between.trim_start().len());
            let end = body_open - (between.len() - between.trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn {
                name,
                return_type,
                ret_start: start,
                ret_end: end,
            })
        }
        Language::Cpp | Language::C | Language::Java => {
            let body_open = prefix.rfind('{')?;
            let prev_close = text[..body_open].rfind(['}', ';']).map_or(0, |p| p + 1);
            let header = &text[prev_close..body_open];
            let open_p = prev_close + header.find('(')?;
            let seg_start = text[prev_close..open_p]
                .rfind(['\n', ';', '}'])
                .map_or(prev_close, |p| prev_close + p + 1);
            let before_paren = text[seg_start..open_p].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.len() < 2 {
                return None;
            }
            let name = words.last().unwrap().to_string();
            let ty_words = &words[..words.len() - 1];
            let ty_str = ty_words.join(" ");
            let start = text[seg_start..open_p].find(ty_words[0])? + seg_start;
            let last = ty_words.last().unwrap();
            let end_rel = text[start..open_p].rfind(last)? + last.len();
            Some(EnclosingFn {
                name,
                return_type: ty_str,
                ret_start: start,
                ret_end: start + end_rel,
            })
        }
    }
}

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
