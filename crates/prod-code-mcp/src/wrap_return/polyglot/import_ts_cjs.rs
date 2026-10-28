/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::clean_lhs_binding;
use super::extract_specifier;
use super::is_ident_boundary;
use super::specifier_matches_decl;

#[cfg(test)]
#[path = "import_ts_cjs/tests.rs"]
mod tests;

pub(crate) fn ts_js_require_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    mask: &[bool],
) -> Vec<String> {
    ts_js_module_call_symbols(content, caller_path, decl_file, fn_name, mask, "require")
}

pub(crate) fn ts_js_dynamic_import_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    mask: &[bool],
) -> Vec<String> {
    ts_js_module_call_symbols(content, caller_path, decl_file, fn_name, mask, "import")
}

fn ts_js_module_call_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    mask: &[bool],
    keyword: &str,
) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut search_idx = 0;
    while let Some(offset) = content[search_idx..].find(keyword) {
        let abs_call_pos = search_idx + offset;
        let before_ok = abs_call_pos == 0 || {
            let prev = content[..abs_call_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_call_pos + keyword.len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_call_pos < mask.len()
            && mask[abs_call_pos]
        {
            let paren_open = abs_call_pos + keyword.len() + (after.len() - trimmed.len());
            if let Some(close) = crate::parameter_object::matching_bracket(content, paren_open) {
                let inside = &content[paren_open + 1..close];
                let specifier = extract_specifier(inside);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let decl_before = content[..abs_call_pos]
                        .rsplit(|c| c == ';' || c == '\n')
                        .next()
                        .unwrap_or("")
                        .trim();
                    let after_close = content[close + 1..].trim_start();
                    if let Some(prop_rest) = after_close.strip_prefix('.') {
                        let prop_rest = prop_rest.trim_start();
                        let prop_name: String = prop_rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                            .collect();
                        if prop_name == fn_name {
                            if let Some((lhs, _)) = decl_before.rsplit_once('=') {
                                let lhs_clean = clean_lhs_binding(lhs);
                                if !lhs_clean.is_empty()
                                    && lhs_clean
                                        .chars()
                                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                                {
                                    symbols.push(lhs_clean.to_string());
                                }
                            }
                        }
                    } else if let Some((lhs, _)) = decl_before.rsplit_once('=') {
                        let lhs = lhs.trim();
                        if let (Some(open), Some(close_brace)) = (lhs.find('{'), lhs.rfind('}')) {
                            if open < close_brace {
                                let inner = &lhs[open + 1..close_brace];
                                for item in inner.split(',') {
                                    let item = item.trim();
                                    if let Some((orig, local)) = item.split_once(':') {
                                        if orig.trim() == fn_name {
                                            symbols.push(local.trim().to_string());
                                        }
                                    } else if item == fn_name {
                                        symbols.push(fn_name.to_string());
                                    }
                                }
                            }
                        } else {
                            let namespace = clean_lhs_binding(lhs);
                            if !namespace.is_empty()
                                && namespace
                                    .chars()
                                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                            {
                                symbols.push(if keyword == "require" {
                                    namespace.to_string()
                                } else {
                                    fn_name.to_string()
                                });
                            }
                        }
                    }
                }
                search_idx = close + 1;
                continue;
            }
        }
        search_idx = abs_call_pos + keyword.len();
    }
    symbols
}

pub(crate) fn ts_js_module_binding_span(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    local_name: &str,
    mask: &[bool],
    keyword: &str,
) -> Option<(usize, usize)> {
    let mut search_idx = 0;
    while let Some(offset) = content[search_idx..].find(keyword) {
        let abs_call_pos = search_idx + offset;
        let before_ok = abs_call_pos == 0 || {
            let prev = content[..abs_call_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_call_pos + keyword.len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_call_pos < mask.len()
            && mask[abs_call_pos]
        {
            let paren_open = abs_call_pos + keyword.len() + (after.len() - trimmed.len());
            if let Some(close) = crate::parameter_object::matching_bracket(content, paren_open) {
                let specifier = extract_specifier(&content[paren_open + 1..close]);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let statement_start = content[..abs_call_pos]
                        .rfind([';', '\n'])
                        .map_or(0, |boundary| boundary + 1);
                    let statement = &content[statement_start..abs_call_pos];
                    let declaration = statement.trim();
                    if let Some((lhs, _)) = declaration.rsplit_once('=') {
                        let lhs = lhs.trim();
                        let lhs_start = statement_start + statement.rfind(lhs).unwrap_or(0);
                        let after_close = content[close + 1..].trim_start();
                        if let Some(prop_rest) = after_close.strip_prefix('.') {
                            let prop_name: String = prop_rest
                                .trim_start()
                                .chars()
                                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                                .collect();
                            if prop_name == fn_name && clean_lhs_binding(lhs) == local_name {
                                return binding_identifier_span(lhs, lhs_start, local_name);
                            }
                        } else if let (Some(open), Some(close_brace)) =
                            (lhs.find('{'), lhs.rfind('}'))
                        {
                            let inner = &lhs[open + 1..close_brace];
                            let mut item_start = 0;
                            for item in inner.split(',') {
                                let item_trimmed = item.trim();
                                let local =
                                    if let Some((original, alias)) = item_trimmed.split_once(':') {
                                        (original.trim() == fn_name).then_some(alias.trim())
                                    } else {
                                        (item_trimmed == fn_name).then_some(fn_name)
                                    };
                                if local == Some(local_name)
                                    && let Some(local_at) = item.find(local_name)
                                {
                                    let start = lhs_start + open + 1 + item_start + local_at;
                                    return Some((start, start + local_name.len()));
                                }
                                item_start += item.len() + 1;
                            }
                        } else if clean_lhs_binding(lhs) == local_name {
                            return binding_identifier_span(lhs, lhs_start, local_name);
                        }
                    }
                }
                search_idx = close + 1;
                continue;
            }
        }
        search_idx = abs_call_pos + keyword.len();
    }
    None
}

fn binding_identifier_span(text: &str, text_start: usize, name: &str) -> Option<(usize, usize)> {
    let at = text.rfind(name)?;
    let end = at + name.len();
    let before = text[..at].chars().next_back();
    let after = text[end..].chars().next();
    if before.is_some_and(|character| !is_ident_boundary(character))
        || after.is_some_and(|character| !is_ident_boundary(character))
    {
        return None;
    }
    Some((text_start + at, text_start + end))
}

pub(crate) fn is_ts_js_require_namespace(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
    mask: &[bool],
) -> bool {
    is_ts_js_module_namespace(content, receiver, caller_path, decl_file, mask, "require")
}

pub(crate) fn is_ts_js_dynamic_import_namespace(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
    mask: &[bool],
) -> bool {
    is_ts_js_module_namespace(content, receiver, caller_path, decl_file, mask, "import")
}

fn is_ts_js_module_namespace(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
    mask: &[bool],
    keyword: &str,
) -> bool {
    let mut search_idx = 0;
    while let Some(offset) = content[search_idx..].find(keyword) {
        let abs_call_pos = search_idx + offset;
        let before_ok = abs_call_pos == 0 || {
            let prev = content[..abs_call_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_call_pos + keyword.len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_call_pos < mask.len()
            && mask[abs_call_pos]
        {
            let paren_open = abs_call_pos + keyword.len() + (after.len() - trimmed.len());
            if let Some(close) = crate::parameter_object::matching_bracket(content, paren_open) {
                let inside = &content[paren_open + 1..close];
                let specifier = extract_specifier(inside);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let decl_before = content[..abs_call_pos]
                        .rsplit(|c| c == ';' || c == '\n')
                        .next()
                        .unwrap_or("")
                        .trim();
                    let after_close = content[close + 1..].trim_start();
                    if !after_close.starts_with('.') {
                        if let Some((lhs, _)) = decl_before.rsplit_once('=') {
                            let lhs_clean = clean_lhs_binding(lhs);
                            if lhs_clean == receiver {
                                return true;
                            }
                        }
                    }
                }
                search_idx = close + 1;
                continue;
            }
        }
        search_idx = abs_call_pos + keyword.len();
    }
    false
}
