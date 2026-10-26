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

use super::super::{cpp_boundary::Boundary, find::find_polyglot_decl};
use crate::parameter_object::Language;

pub(crate) fn c_cpp_proves_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> bool {
    let Ok(text) = std::fs::read_to_string(decl_file) else {
        return false;
    };
    let lang = Language::of(decl_file).unwrap_or(Language::Cpp);
    let Ok(decl) = find_polyglot_decl(&text, lang, Some(fn_name), None) else {
        return false;
    };
    // This legacy import query has no workspace root. Keep its search bounded
    // to the declaring directory; the public wrapper supplies the actual root.
    Boundary::new(
        decl_file.parent().unwrap_or(decl_file),
        decl_file,
        &text,
        &decl,
    )
    .imports(caller_path, content)
}

#[cfg(test)]
#[path = "import_cpp_tests.rs"]
mod tests;
