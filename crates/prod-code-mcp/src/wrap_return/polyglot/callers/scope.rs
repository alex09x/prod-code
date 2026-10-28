/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::import::mask_proven_import_binding;
use super::super::shadow::is_locally_shadowed;
use crate::parameter_object::Language;
use std::path::Path;

pub(super) fn is_shadowed(
    content: &str,
    code: &str,
    at: usize,
    name: &str,
    lang: Language,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> bool {
    if matches!(lang, Language::TypeScript | Language::JavaScript) {
        let shadow_code =
            mask_proven_import_binding(content, code, caller_path, decl_file, fn_name, name, lang);
        is_locally_shadowed(shadow_code.as_deref().unwrap_or(code), at, name, lang)
    } else {
        is_locally_shadowed(content, at, name, lang)
    }
}
