/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::is_ident;
use crate::parameter_object::Language;

/// Finds the end of a Python function body based on indentation.
pub(crate) fn find_python_body_close(text: &str, colon_pos: usize) -> usize {
    let line_start = text[..colon_pos].rfind('\n').map_or(0, |p| p + 1);
    let def_line = &text[line_start..colon_pos];
    let def_indent = def_line.len() - def_line.trim_start().len();

    let rest = &text[colon_pos + 1..];
    let mut current_offset = colon_pos + 1;
    for line in rest.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            current_offset += line.len() + 1;
            continue;
        }
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= def_indent {
            return current_offset;
        }
        current_offset += line.len() + 1;
    }
    text.len()
}

/// Checks whether all usages of `var_name` in `scope` only call/access methods in `extracted_methods`.
pub(crate) fn verify_all_usages_safe(
    text: &str,
    scope_start: usize,
    scope_end: usize,
    var_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
) -> bool {
    let scope_text = &text[scope_start..scope_end];
    let var_bytes = var_name.as_bytes();

    let mut i = 0usize;
    while i < scope_text.len() {
        let abs_pos = scope_start + i;
        if abs_pos + var_bytes.len() <= text.len()
            && &text.as_bytes()[abs_pos..abs_pos + var_bytes.len()] == var_bytes
            && mask[abs_pos]
        {
            // Boundary checks: previous character
            let prev_ok = if abs_pos == 0 {
                true
            } else {
                let prev = text.as_bytes()[abs_pos - 1];
                !is_ident(prev as char) && prev != b'.'
            };

            // Boundary checks: next character
            let next_pos = abs_pos + var_bytes.len();
            let next_ok = if next_pos >= text.len() {
                true
            } else {
                let next = text.as_bytes()[next_pos];
                !is_ident(next as char)
            };

            if prev_ok && next_ok {
                // Inspect what follows var_name (skipping whitespace)
                let after = &text[next_pos..scope_end];
                let trimmed = after.trim_start();

                let member_opt = match lang {
                    Language::Cpp | Language::C => trimmed
                        .strip_prefix("->")
                        .or_else(|| trimmed.strip_prefix('.'))
                        .map(str::trim_start),
                    Language::TypeScript | Language::JavaScript => trimmed
                        .strip_prefix("?.")
                        .or_else(|| trimmed.strip_prefix('.'))
                        .map(str::trim_start),
                    _ => trimmed.strip_prefix('.').map(str::trim_start),
                };

                let Some(member_str) = member_opt else {
                    // Used as a bare value, passed as argument, returned, or assigned -> unsafe to migrate!
                    return false;
                };

                // Extract the member identifier name
                let member_name: String = member_str.chars().take_while(|c| is_ident(*c)).collect();
                if member_name.is_empty() {
                    return false;
                }

                if !extracted_methods.iter().any(|m| m == &member_name) {
                    // Accessed an unextracted method or field -> unsafe to migrate!
                    return false;
                }
            }
        }
        i += 1;
    }

    true
}
