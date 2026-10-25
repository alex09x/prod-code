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

use super::super::import::{find_dotted_receiver_span, is_proven_namespace_import};
use super::super::shadow::is_locally_shadowed;
use crate::parameter_object::Language;

pub(super) enum Receiver {
    Bare,
    Selected(usize),
    Unrelated,
    Ambiguous(usize),
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
            if is_locally_shadowed(code, start, root, lang) {
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
