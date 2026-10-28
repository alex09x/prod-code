/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::import::imported_caller_symbols;
use crate::parameter_object::Language;
use std::path::Path;

pub(super) fn discover_symbols_to_search(
    content: &str,
    caller_path: &Path,
    selected_path: &Path,
    selected_name: &str,
    lang: Language,
    is_decl_file: bool,
    has_semantic_references: bool,
) -> Vec<String> {
    let js = matches!(lang, Language::TypeScript | Language::JavaScript);
    let mut symbols =
        if is_decl_file || matches!(lang, Language::Swift | Language::Cpp | Language::C) {
            if !content.contains(selected_name) {
                return Vec::new();
            }
            vec![selected_name.to_string()]
        } else {
            imported_caller_symbols(content, caller_path, selected_path, selected_name, lang)
        };
    if has_semantic_references && js && !is_decl_file {
        symbols.push(selected_name.to_string());
    }
    symbols.retain(|symbol| content.contains(symbol));
    symbols.sort();
    symbols.dedup();
    symbols
}
