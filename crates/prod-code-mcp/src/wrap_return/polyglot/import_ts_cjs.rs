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

pub(crate) fn ts_js_require_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    mask: &[bool],
) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut search_idx = 0;
    while let Some(req_offset) = content[search_idx..].find("require") {
        let abs_req_pos = search_idx + req_offset;
        let before_ok = abs_req_pos == 0 || {
            let prev = content[..abs_req_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_req_pos + "require".len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_req_pos < mask.len()
            && mask[abs_req_pos]
        {
            let paren_open = abs_req_pos + "require".len() + (after.len() - trimmed.len());
            let mut p_depth = 0usize;
            let mut close_paren = None;
            for (offset, c) in content[paren_open..].char_indices() {
                match c {
                    '(' => p_depth += 1,
                    ')' => {
                        p_depth -= 1;
                        if p_depth == 0 {
                            close_paren = Some(paren_open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close_paren {
                let inside = &content[paren_open + 1..close];
                let specifier = extract_specifier(inside);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let decl_before = content[..abs_req_pos]
                        .rsplit(';')
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
                            let lhs_clean = clean_lhs_binding(lhs);
                            if !lhs_clean.is_empty()
                                && lhs_clean
                                    .chars()
                                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                            {
                                symbols.push(fn_name.to_string());
                            }
                        }
                    }
                }
                search_idx = close + 1;
                continue;
            }
        }
        search_idx = abs_req_pos + "require".len();
    }
    symbols
}

pub(crate) fn is_ts_js_require_namespace(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
    mask: &[bool],
) -> bool {
    let mut search_idx = 0;
    while let Some(req_offset) = content[search_idx..].find("require") {
        let abs_req_pos = search_idx + req_offset;
        let before_ok = abs_req_pos == 0 || {
            let prev = content[..abs_req_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_req_pos + "require".len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_req_pos < mask.len()
            && mask[abs_req_pos]
        {
            let paren_open = abs_req_pos + "require".len() + (after.len() - trimmed.len());
            let mut p_depth = 0usize;
            let mut close_paren = None;
            for (offset, c) in content[paren_open..].char_indices() {
                match c {
                    '(' => p_depth += 1,
                    ')' => {
                        p_depth -= 1;
                        if p_depth == 0 {
                            close_paren = Some(paren_open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close_paren {
                let inside = &content[paren_open + 1..close];
                let specifier = extract_specifier(inside);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let decl_before = content[..abs_req_pos]
                        .rsplit(';')
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
        search_idx = abs_req_pos + "require".len();
    }
    false
}
