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

use super::super::import::{
    find_dotted_receiver_span, is_proven_namespace_import, mask_proven_import_binding,
};
use super::super::shadow::is_locally_shadowed;
use crate::parameter_object::Language;
use crate::wrap_return::utils::is_ident;

pub(super) enum Receiver {
    Bare,
    Selected(usize),
    Unrelated,
    Ambiguous(usize),
}

pub(super) fn call_symbol_site(
    content: &str,
    at: usize,
    search_symbol: &str,
    selected_name: &str,
    lang: Language,
) -> Option<(usize, String)> {
    if search_symbol == selected_name {
        return Some((at, selected_name.to_string()));
    }
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return Some((at, search_symbol.to_string()));
    }
    let after = &content[at + search_symbol.len()..];
    let trimmed = after.trim_start();
    if trimmed.starts_with("?.") || trimmed.starts_with('.') {
        let operator_len = if trimmed.starts_with("?.") { 2 } else { 1 };
        let after_operator = &trimmed[operator_len..];
        let member_text = after_operator.trim_start();
        let member_name: String = member_text
            .chars()
            .take_while(|character| is_ident(*character) || *character == '$')
            .collect();
        if member_name == selected_name {
            let whitespace_before = after.len() - trimmed.len();
            let whitespace_after = after_operator.len() - member_text.len();
            let member_at =
                at + search_symbol.len() + whitespace_before + operator_len + whitespace_after;
            return Some((member_at, selected_name.to_string()));
        }
        return None;
    }
    trimmed
        .starts_with('(')
        .then(|| (at, search_symbol.to_string()))
}

/// A same-file bare binding and a member property require different identity evidence.
/// Semantic candidates are filtered by exact reference position by the caller; without
/// that evidence an unknown same-file receiver must remain visible as an incomplete plan.
pub(super) fn receiver(
    content: &str,
    code: &str,
    at: usize,
    lang: Language,
    files: (&Path, &Path),
    is_decl_file: bool,
    has_semantic_references: bool,
) -> Receiver {
    let before = code[..at].trim_end();
    let Some(before_dot) = before
        .strip_suffix("?.")
        .or_else(|| before.strip_suffix("->"))
        .or_else(|| before.strip_suffix('.'))
    else {
        return Receiver::Bare;
    };
    let before_dot = before_dot.trim_end();
    let (relative_start, receiver) = find_dotted_receiver_span(before_dot);
    let start = before_dot.as_ptr() as usize - code.as_ptr() as usize + relative_start;
    if !has_semantic_references {
        let namespace = is_proven_namespace_import(content, receiver, files.0, files.1, lang);
        if namespace && matches!(lang, Language::TypeScript | Language::JavaScript) {
            let root = receiver.split('.').next().unwrap_or(receiver);
            let member: String = content[at..]
                .chars()
                .take_while(|character| is_ident(*character) || *character == '$')
                .collect();
            let shadow_code =
                mask_proven_import_binding(content, code, files.0, files.1, &member, root, lang);
            if is_locally_shadowed(shadow_code.as_deref().unwrap_or(code), start, root, lang) {
                return Receiver::Unrelated;
            }
        }
        if !namespace {
            if !is_decl_file {
                return Receiver::Unrelated;
            }
            if matches!(lang, Language::TypeScript | Language::JavaScript) {
                return Receiver::Ambiguous(start);
            }
        }
    }
    Receiver::Selected(start)
}
