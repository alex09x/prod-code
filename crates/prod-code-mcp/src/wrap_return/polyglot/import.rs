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

use crate::parameter_object::Language;

/// Proves whether `caller_path` genuinely imports or shares scope with `decl_file` for `fn_name`.
/// Used when semantic references are empty to prevent cross-file text fallback from rewriting
/// unrelated local aliases or same-named symbols in other files (#982).
#[cfg(test)]
pub(crate) fn proves_cross_file_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> bool {
    !imported_caller_symbols(content, caller_path, decl_file, fn_name, lang).is_empty()
}

/// Finds the local symbol name(s) under which `decl_file::fn_name` is imported or accessible in `caller_path`.
/// Returns an empty list if `caller_path` does not import or share scope with `decl_file` for `fn_name`.
pub(crate) fn imported_caller_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> Vec<String> {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if decl_stem.is_empty() {
        return Vec::new();
    }
    match lang {
        Language::TypeScript | Language::JavaScript => {
            ts_js_imported_symbols(content, caller_path, decl_file, fn_name)
        }
        Language::Python => python_imported_symbols(content, caller_path, decl_file, fn_name),
        Language::Go if caller_path.parent() == decl_file.parent() => {
            vec![fn_name.to_string()]
        }
        Language::Go => go_imported_symbols(content, caller_path, decl_file, fn_name),
        Language::Swift if caller_path.parent() == decl_file.parent() => vec![fn_name.to_string()],
        Language::Cpp | Language::C
            if c_cpp_proves_import(content, caller_path, decl_file, fn_name) =>
        {
            vec![fn_name.to_string()]
        }
        _ => Vec::new(),
    }
}

#[path = "import_cpp.rs"]
mod import_cpp;
use import_cpp::c_cpp_proves_import;

#[path = "import_go.rs"]
mod import_go;
use import_go::{go_imported_symbols, is_go_namespace_import};

#[path = "import_python.rs"]
mod import_python;
use import_python::python_imported_symbols;

#[path = "import_ts.rs"]
mod import_ts;
use import_ts::ts_js_imported_symbols;

#[cfg(test)]
#[path = "review_import_tests.rs"]
mod review_import_tests;

pub(crate) fn is_proven_namespace_import(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
    lang: Language,
) -> bool {
    if receiver.is_empty() {
        return false;
    }
    match lang {
        Language::TypeScript | Language::JavaScript => {
            import_ts::is_ts_js_namespace_import(content, receiver, caller_path, decl_file)
        }
        Language::Python => {
            import_python::is_python_namespace_import(content, receiver, caller_path, decl_file)
        }
        Language::Go => is_go_namespace_import(content, receiver, caller_path, decl_file),
        _ => false,
    }
}

pub(crate) fn find_dotted_receiver_span<'a>(before_dot: &'a str) -> (usize, &'a str) {
    let bytes = before_dot.as_bytes();
    let mut i = before_dot.len();
    let mut start = before_dot.len();
    while i > 0 {
        let ident_end = i;
        while i > 0 {
            let ch = before_dot[..i].chars().next_back().unwrap();
            if crate::wrap_return::utils::is_ident(ch) {
                i -= ch.len_utf8();
            } else {
                break;
            }
        }
        if i == ident_end {
            break;
        }
        start = i;
        if i > 0 && bytes[i - 1] == b'.' && (i < 2 || bytes[i - 2] != b'.') {
            i -= 1;
        } else if i >= 2 && &bytes[i - 2..i] == b"::" {
            i -= 2;
        } else if i >= 2 && &bytes[i - 2..i] == b"->" {
            i -= 2;
        } else {
            break;
        }
    }
    (start, &before_dot[start..])
}

pub(crate) fn extract_specifier(s: &str) -> &str {
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start = i + 1;
            for (j, end_c) in chars {
                if end_c == quote {
                    return &s[start..j];
                }
            }
            return &s[start..];
        } else if c == '<' {
            let start = i + 1;
            for (j, end_c) in chars {
                if end_c == '>' {
                    return &s[start..j];
                }
            }
            return &s[start..];
        }
    }
    ""
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
